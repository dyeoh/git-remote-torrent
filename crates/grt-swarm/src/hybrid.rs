use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use grt_core::{InfoHash, InfoHashV2};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::error::Error;

/// The file name inside every pack torrent; part of the hashed metadata.
pub(crate) const PACK_FILE: &str = "pack";

/// BEP 52 hashes 16 KiB blocks into each file's merkle tree.
const BLOCK_LEN: usize = 16 * 1024;

/// Smallest piece length; doubled until a pack has at most [`MAX_PIECES`].
const MIN_PIECE_LEN: u64 = 256 * 1024;
const MAX_PIECES: u64 = 1024;

/// A pack described as a BEP 52 hybrid torrent: one info dictionary with
/// both v1 (SHA-1 pieces) and v2 (SHA-256 merkle tree) metadata, named
/// `pack`. v1 clients find it by [`HybridTorrent::v1`], v2 clients by
/// [`HybridTorrent::v2`].
#[derive(Debug, Clone)]
pub struct HybridTorrent {
    /// The `.torrent` file, with `piece layers` when the pack spans several
    /// pieces.
    pub bytes: Vec<u8>,
    /// SHA-1 of the info dictionary.
    pub v1: InfoHash,
    /// SHA-256 of the same info dictionary.
    pub v2: InfoHashV2,
}

/// The piece length for a pack of `len` bytes. Fixed by the format so anyone
/// can recompute a pack's infohashes from its bytes (ADR-0008).
pub fn piece_length(len: u64) -> u64 {
    let mut piece = MIN_PIECE_LEN;
    while len.div_ceil(piece) > MAX_PIECES {
        piece *= 2;
    }
    piece
}

/// Describes the pack file at `path` as a hybrid torrent, reading it once.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be read, or
/// [`Error::Torrent`] if it is empty (packs never are).
pub fn hybrid_torrent(path: &Path) -> Result<HybridTorrent, Error> {
    let len = std::fs::metadata(path)?.len();
    if len == 0 {
        return Err(Error::Torrent(format!("{} is empty", path.display())));
    }
    let piece = piece_length(len);
    let blocks_per_piece = usize::try_from(piece).map_err(too_large)? / BLOCK_LEN;

    let mut reader = BufReader::new(File::open(path)?);
    let mut leaves: Vec<[u8; 32]> = Vec::new();
    let mut pieces: Vec<u8> = Vec::new();
    let mut piece_hash = Sha1::new();
    let mut blocks_in_piece = 0;
    let mut block = vec![0; BLOCK_LEN];
    loop {
        let filled = read_block(&mut reader, &mut block)?;
        if filled == 0 {
            break;
        }
        leaves.push(Sha256::digest(&block[..filled]).into());
        piece_hash.update(&block[..filled]);
        blocks_in_piece += 1;
        if blocks_in_piece == blocks_per_piece {
            pieces.extend_from_slice(&piece_hash.finalize_reset());
            blocks_in_piece = 0;
        }
    }
    if blocks_in_piece > 0 {
        pieces.extend_from_slice(&piece_hash.finalize_reset());
    }

    let pieces_root = merkle_root(&leaves, leaves.len().next_power_of_two());
    let info = info_dict(len, piece, &pieces_root, &pieces);
    let mut bytes = b"d4:info".to_vec();
    bytes.extend_from_slice(&info);
    if len > piece {
        // One hash per piece: the subtree root over that piece's blocks.
        let layer: Vec<u8> = leaves
            .chunks(blocks_per_piece)
            .flat_map(|chunk| merkle_root(chunk, blocks_per_piece))
            .collect();
        bytes.extend_from_slice(b"12:piece layersd");
        byte_string(&mut bytes, &pieces_root);
        byte_string(&mut bytes, &layer);
        bytes.push(b'e');
    }
    bytes.push(b'e');
    Ok(HybridTorrent {
        bytes,
        v1: InfoHash::from(<[u8; 20]>::from(Sha1::digest(&info))),
        v2: InfoHashV2::from(<[u8; 32]>::from(Sha256::digest(&info))),
    })
}

/// The canonical bencoded info dictionary of a one-file hybrid torrent named
/// `pack`; keys in sorted order.
fn info_dict(len: u64, piece: u64, pieces_root: &[u8; 32], pieces: &[u8]) -> Vec<u8> {
    let mut info = b"d9:file treed4:packd0:d6:lengthi".to_vec();
    info.extend_from_slice(format!("{len}e11:pieces root").as_bytes());
    byte_string(&mut info, pieces_root);
    // Close the "" entry, the file's dictionary and the file tree.
    info.extend_from_slice(b"eee");
    info.extend_from_slice(format!("6:lengthi{len}e12:meta versioni2e4:name4:pack").as_bytes());
    info.extend_from_slice(format!("12:piece lengthi{piece}e6:pieces").as_bytes());
    byte_string(&mut info, pieces);
    info.push(b'e');
    info
}

/// Root of a merkle tree over `leaves`, padded with zero hashes to `width`
/// leaves, as BEP 52 pads past the end of a file.
fn merkle_root(leaves: &[[u8; 32]], width: usize) -> [u8; 32] {
    let mut level = leaves.to_vec();
    level.resize(width.max(1), [0; 32]);
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| {
                let mut hash = Sha256::new();
                hash.update(pair[0]);
                hash.update(pair[1]);
                hash.finalize().into()
            })
            .collect();
    }
    level[0]
}

/// Fills `block` as far as the file allows; returns how many bytes it holds.
fn read_block(reader: &mut impl Read, block: &mut [u8]) -> Result<usize, Error> {
    let mut filled = 0;
    while filled < block.len() {
        match reader.read(&mut block[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn byte_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(format!("{}:", bytes.len()).as_bytes());
    out.extend_from_slice(bytes);
}

fn too_large(_: std::num::TryFromIntError) -> Error {
    Error::Torrent("piece length does not fit in memory".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic contents shared with the libtorrent script that produced
    /// the expected hashes.
    fn data(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from((i * 7 + i / 251) % 256).unwrap())
            .collect()
    }

    #[test]
    fn matches_libtorrent() {
        // libtorrent 2.1.1 `create_torrent` (hybrid by default) on the same
        // bytes, named `pack`, with the same piece length.
        let cases = [
            (
                100,
                "c7c5f3bfba6a9a8f1425bfc6259520200d0cd0bc",
                "65398891debc773d3b0c338f1ab9420c11fd4d6a0eeaa1cb24ba1f03dae3dcd2",
            ),
            (
                70_402,
                "8ff9b11d5b6af47dac590384867d17162e7d8cae",
                "7c64a33cc6ab09b5941e70b9c95fb6143adb63d79fca9443bd079939522ab1ba",
            ),
            (
                1_000_000,
                "e3def02d140a8b60060a17f6e8e3b78ee7c41f42",
                "70b88bdd6dc1428dc2d90bf8de7d62cd1f7ac42cd5c527a7b1febfb32e4a95cb",
            ),
        ];
        let dir = tempfile::TempDir::new().unwrap();
        for (len, v1, v2) in cases {
            let path = dir.path().join(format!("pack-{len}"));
            std::fs::write(&path, data(len)).unwrap();
            let torrent = hybrid_torrent(&path).unwrap();
            assert_eq!(torrent.v1.to_string(), v1, "v1 infohash of {len} bytes");
            assert_eq!(torrent.v2.to_string(), v2, "v2 infohash of {len} bytes");
        }
    }

    #[test]
    fn piece_length_keeps_packs_to_1024_pieces() {
        assert_eq!(piece_length(1), 256 * 1024);
        assert_eq!(
            piece_length(256 << 20),
            256 * 1024,
            "256 MiB is 1024 pieces"
        );
        assert_eq!(piece_length((256 << 20) + 1), 512 * 1024);
    }
}
