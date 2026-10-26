use grt_core::ObjectId;
use grt_upstream::{ListedRef, PushResult, PushSpec, TransferOptions};

use crate::error::Error;

/// A ref git asked for in a `fetch` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRef {
    /// The object the ref pointed to in `list`.
    pub id: ObjectId,
    /// The ref name, `HEAD` or under `refs/`.
    pub name: String,
}

/// What the remote-helper protocol needs from a remote. [`crate::Router`]
/// is the real one; tests substitute fakes.
pub trait Remote {
    /// Lists the remote's refs, including `HEAD` as a symbolic ref if known.
    ///
    /// # Errors
    ///
    /// Returns an error if no source can list the refs.
    fn list(&mut self, options: &TransferOptions) -> Result<Vec<ListedRef>, Error>;

    /// Brings the objects of `refs` into the local repository. Git updates
    /// the refs itself afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error if the objects cannot be fetched from any source.
    fn fetch(&mut self, refs: &[FetchRef], options: &TransferOptions) -> Result<(), Error>;

    /// Pushes `specs` and reports each ref's outcome.
    ///
    /// # Errors
    ///
    /// Returns an error if the push cannot be attempted at all.
    fn push(
        &mut self,
        specs: &[PushSpec],
        options: &TransferOptions,
    ) -> Result<Vec<PushResult>, Error>;
}
