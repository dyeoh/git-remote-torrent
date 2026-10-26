use grt_core::{ObjectId, SignedManifest};
use grt_upstream::{ListedRef, PushResult, PushSpec, PushStatus, TransferOptions, Upstream};

use crate::error::Error;
use crate::remote::{FetchRef, Remote};
use crate::swarm::Swarm;

/// Where `list` got its refs, which decides where `fetch` gets objects.
enum Listed {
    Upstream,
    Swarm(Box<SignedManifest>),
}

/// Chooses between the upstream and the swarm for each command.
///
/// While the upstream answers, its refs are canonical. A fetch still goes to
/// the swarm first when the manifest covers every wanted object, and falls
/// back to the upstream if the swarm fails. When the upstream cannot list its
/// refs, the manifest serves both `list` and `fetch`. Pushes go only to the
/// upstream.
pub struct Router<U, S> {
    upstream: Option<U>,
    swarm: Option<S>,
    listed: Option<Listed>,
}

impl<U: Upstream, S: Swarm> Router<U, S> {
    /// A `torrent::<url>` remote: `upstream`, backed by `swarm` if the remote
    /// has a torrent address.
    pub fn hybrid(upstream: U, swarm: Option<S>) -> Self {
        Router {
            upstream: Some(upstream),
            swarm,
            listed: None,
        }
    }

    /// A `torrent://<key>` remote, served by the swarm alone.
    pub fn swarm(swarm: S) -> Self {
        Router {
            upstream: None,
            swarm: Some(swarm),
            listed: None,
        }
    }

    /// Lists the manifest's refs and remembers it for `fetch`.
    fn list_manifest(&mut self, manifest: SignedManifest) -> Vec<ListedRef> {
        let mut refs = Vec::new();
        if let Some(head) = manifest.manifest().head() {
            refs.push(ListedRef::symbolic("HEAD", head.as_str()));
        }
        for (name, id) in manifest.manifest().refs() {
            refs.push(ListedRef::object(name.as_str(), *id));
        }
        self.listed = Some(Listed::Swarm(Box::new(manifest)));
        refs
    }

    /// Fetches from the swarm if its manifest covers `wants`. Returns false,
    /// after saying why on stderr, if the upstream should be used instead.
    fn try_swarm_fetch(&mut self, wants: &[ObjectId]) -> bool {
        let Some(swarm) = &mut self.swarm else {
            return false;
        };
        let manifest = match swarm.manifest() {
            Ok(Some(manifest)) if covers(&manifest, wants) => manifest,
            // The upstream is ahead of the manifest, or nothing is published.
            Ok(_) => return false,
            Err(error) => {
                eprintln!("warning: could not resolve the manifest ({error}); using the upstream");
                return false;
            }
        };
        match swarm.fetch(&manifest, wants) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("warning: fetching from the swarm failed ({error}); using the upstream");
                false
            }
        }
    }
}

impl<U: Upstream, S: Swarm> Remote for Router<U, S> {
    fn list(&mut self, options: &TransferOptions) -> Result<Vec<ListedRef>, Error> {
        let Some(upstream) = &self.upstream else {
            let swarm = self.swarm.as_mut().ok_or_else(no_source)?;
            let manifest = swarm.manifest()?.ok_or_else(|| {
                Error::Unavailable("no manifest is published at this address".to_owned())
            })?;
            return Ok(self.list_manifest(manifest));
        };
        let upstream_error = match upstream.list(options) {
            Ok(refs) => {
                self.listed = Some(Listed::Upstream);
                return Ok(refs);
            }
            Err(error) => error,
        };
        let Some(swarm) = &mut self.swarm else {
            return Err(upstream_error.into());
        };
        match swarm.manifest() {
            Ok(Some(manifest)) => {
                eprintln!(
                    "warning: the upstream is unavailable ({upstream_error})\n\
                     warning: serving manifest #{} from the swarm",
                    manifest.seq().get()
                );
                Ok(self.list_manifest(manifest))
            }
            Ok(None) => Err(upstream_error.into()),
            Err(swarm_error) => Err(Error::Unavailable(format!(
                "{upstream_error}; the swarm is unavailable too ({swarm_error})"
            ))),
        }
    }

    fn fetch(&mut self, refs: &[FetchRef], options: &TransferOptions) -> Result<(), Error> {
        let wants: Vec<ObjectId> = refs.iter().map(|wanted| wanted.id).collect();
        if let Some(Listed::Swarm(manifest)) = &self.listed {
            let swarm = self.swarm.as_mut().ok_or_else(no_source)?;
            return swarm.fetch(manifest, &wants);
        }
        if self.try_swarm_fetch(&wants) {
            return Ok(());
        }
        let upstream = self.upstream.as_ref().ok_or_else(no_source)?;
        let names: Vec<String> = refs.iter().map(|wanted| wanted.name.clone()).collect();
        Ok(upstream.fetch(&names, options)?)
    }

    fn push(
        &mut self,
        specs: &[PushSpec],
        options: &TransferOptions,
    ) -> Result<Vec<PushResult>, Error> {
        let swarm = self.swarm.as_mut().filter(|swarm| swarm.publishes());
        let Some(upstream) = &self.upstream else {
            // A pure swarm remote: the manifest is the only copy of the refs.
            return match swarm {
                Some(swarm) => swarm.push(specs, true),
                None => Ok(reject_all(
                    specs,
                    "this torrent:// remote is not yours to publish; if you hold its key, \
                     set remote.<name>.torrentPublish=true",
                )),
            };
        };
        let results = match upstream.push(specs, options) {
            Ok(results) => results,
            Err(error) => {
                let Some(swarm) = swarm else {
                    return Err(error.into());
                };
                // The manifest becomes the only up-to-date copy until reconciled.
                eprintln!(
                    "warning: the upstream is unavailable ({error})\n\
                     warning: pushing to the swarm only; run `git torrent reconcile` \
                     once the upstream is back"
                );
                let results = swarm.push(specs, true)?;
                let pushed: Vec<String> = results
                    .iter()
                    .filter(|result| result.status == PushStatus::Ok)
                    .map(|result| result.dst.clone())
                    .collect();
                swarm.record_unsynced(&pushed)?;
                return Ok(results);
            }
        };
        if let Some(swarm) = swarm {
            // The upstream already enforced its rules; mirror what it accepted.
            let accepted: Vec<PushSpec> = specs
                .iter()
                .filter(|spec| {
                    results
                        .iter()
                        .any(|result| result.dst == spec.dst && result.status == PushStatus::Ok)
                })
                .cloned()
                .collect();
            if !accepted.is_empty() {
                match swarm.push(&accepted, false) {
                    Ok(signed) => {
                        for result in signed {
                            if let PushStatus::Rejected(reason) = result.status {
                                eprintln!(
                                    "warning: pushed {} to the upstream, but not to the swarm: \
                                     {reason}",
                                    result.dst
                                );
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("warning: pushed to the upstream, but not to the swarm: {error}");
                    }
                }
            }
        }
        Ok(results)
    }
}

fn reject_all(specs: &[PushSpec], reason: &str) -> Vec<PushResult> {
    specs
        .iter()
        .map(|spec| PushResult {
            dst: spec.dst.clone(),
            status: PushStatus::Rejected(reason.to_owned()),
        })
        .collect()
}

/// True if every object in `wants` is a ref tip in `manifest`.
fn covers(manifest: &SignedManifest, wants: &[ObjectId]) -> bool {
    wants
        .iter()
        .all(|want| manifest.manifest().refs().values().any(|tip| tip == want))
}

fn no_source() -> Error {
    Error::Unavailable("this remote has no source for the request".to_owned())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use grt_core::{Manifest, PublisherKey, Seq};

    use super::*;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const B: &str = "2e65efe2a145dda7ee51d1741299f848e5bf752e";

    type Log = Rc<RefCell<Vec<String>>>;

    struct FakeUpstream {
        log: Log,
        up: bool,
    }

    impl Upstream for FakeUpstream {
        fn list(&self, _options: &TransferOptions) -> Result<Vec<ListedRef>, grt_upstream::Error> {
            self.log.borrow_mut().push("upstream list".to_owned());
            if !self.up {
                return Err(grt_upstream::Error::UnexpectedOutput {
                    command: "ls-remote",
                    line: "repository not found".to_owned(),
                });
            }
            Ok(vec![ListedRef::object(
                "refs/heads/main",
                B.parse().unwrap(),
            )])
        }

        fn fetch(
            &self,
            refs: &[String],
            _options: &TransferOptions,
        ) -> Result<(), grt_upstream::Error> {
            self.log
                .borrow_mut()
                .push(format!("upstream fetch {refs:?}"));
            Ok(())
        }

        fn push(
            &self,
            specs: &[PushSpec],
            _options: &TransferOptions,
        ) -> Result<Vec<PushResult>, grt_upstream::Error> {
            self.log.borrow_mut().push("upstream push".to_owned());
            if !self.up {
                return Err(grt_upstream::Error::UnexpectedOutput {
                    command: "push",
                    line: "could not connect".to_owned(),
                });
            }
            Ok(specs
                .iter()
                .map(|spec| PushResult {
                    dst: spec.dst.clone(),
                    status: PushStatus::Ok,
                })
                .collect())
        }
    }

    struct FakeSwarm {
        log: Log,
        manifest: Option<SignedManifest>,
        fail: bool,
        publishes: bool,
    }

    impl Swarm for FakeSwarm {
        fn manifest(&mut self) -> Result<Option<SignedManifest>, Error> {
            self.log.borrow_mut().push("swarm manifest".to_owned());
            Ok(self.manifest.clone())
        }

        fn fetch(&mut self, manifest: &SignedManifest, wants: &[ObjectId]) -> Result<(), Error> {
            self.log.borrow_mut().push(format!(
                "swarm fetch #{} {}",
                manifest.seq().get(),
                wants.len()
            ));
            if self.fail {
                return Err(Error::Unavailable("no peers".to_owned()));
            }
            Ok(())
        }

        fn publishes(&self) -> bool {
            self.publishes
        }

        fn push(
            &mut self,
            specs: &[PushSpec],
            fast_forward_only: bool,
        ) -> Result<Vec<PushResult>, Error> {
            let dsts: Vec<&str> = specs.iter().map(|spec| spec.dst.as_str()).collect();
            self.log
                .borrow_mut()
                .push(format!("swarm push {dsts:?} ff={fast_forward_only}"));
            Ok(specs
                .iter()
                .map(|spec| PushResult {
                    dst: spec.dst.clone(),
                    status: PushStatus::Ok,
                })
                .collect())
        }

        fn record_unsynced(&mut self, refs: &[String]) -> Result<(), Error> {
            self.log
                .borrow_mut()
                .push(format!("swarm unsynced {refs:?}"));
            Ok(())
        }
    }

    /// A manifest with `main` at `tip` and `HEAD` pointing to it.
    fn signed(tip: &str) -> SignedManifest {
        let mut manifest = Manifest::new();
        manifest
            .insert_ref("refs/heads/main".parse().unwrap(), tip.parse().unwrap())
            .unwrap();
        manifest
            .set_head(Some("refs/heads/main".parse().unwrap()))
            .unwrap();
        PublisherKey::generate()
            .unwrap()
            .sign(&manifest, Seq::new(3), b"")
            .unwrap()
    }

    fn router(
        up: bool,
        manifest: Option<SignedManifest>,
        fail: bool,
    ) -> (Router<FakeUpstream, FakeSwarm>, Log) {
        let log = Log::default();
        let swarm = FakeSwarm {
            log: log.clone(),
            manifest,
            fail,
            publishes: true,
        };
        let upstream = FakeUpstream {
            log: log.clone(),
            up,
        };
        (Router::hybrid(upstream, Some(swarm)), log)
    }

    fn want(id: &str) -> Vec<FetchRef> {
        vec![FetchRef {
            id: id.parse().unwrap(),
            name: "refs/heads/main".to_owned(),
        }]
    }

    fn names(refs: &[ListedRef]) -> Vec<String> {
        refs.iter().map(|listed| listed.name.clone()).collect()
    }

    #[test]
    fn live_upstream_lists_and_swarm_fetches_when_the_manifest_matches() {
        let options = TransferOptions::default();
        let (mut router, log) = router(true, Some(signed(B)), false);
        router.list(&options).unwrap();
        router.fetch(&want(B), &options).unwrap();
        assert_eq!(
            *log.borrow(),
            ["upstream list", "swarm manifest", "swarm fetch #3 1"],
            "upstream refs, swarm objects"
        );
    }

    #[test]
    fn live_upstream_ahead_of_the_manifest_fetches_from_upstream() {
        let options = TransferOptions::default();
        let (mut router, log) = router(true, Some(signed(A)), false);
        router.list(&options).unwrap();
        router.fetch(&want(B), &options).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                "upstream list",
                "swarm manifest",
                r#"upstream fetch ["refs/heads/main"]"#
            ],
            "a stale manifest is not used"
        );
    }

    #[test]
    fn swarm_failure_falls_back_to_upstream() {
        let options = TransferOptions::default();
        let (mut router, log) = router(true, Some(signed(B)), true);
        router.list(&options).unwrap();
        router.fetch(&want(B), &options).unwrap();
        assert_eq!(
            log.borrow().last().map(String::as_str),
            Some(r#"upstream fetch ["refs/heads/main"]"#),
            "the upstream finishes what the swarm could not"
        );
    }

    #[test]
    fn dead_upstream_is_served_from_the_manifest() {
        let options = TransferOptions::default();
        let (mut router, log) = router(false, Some(signed(A)), false);
        let refs = router.list(&options).unwrap();
        assert_eq!(names(&refs), ["HEAD", "refs/heads/main"], "manifest refs");
        router.fetch(&want(A), &options).unwrap();
        assert_eq!(
            *log.borrow(),
            ["upstream list", "swarm manifest", "swarm fetch #3 1"],
            "no second upstream attempt"
        );
    }

    #[test]
    fn dead_upstream_without_a_manifest_fails() {
        let (mut router, _) = router(false, None, false);
        assert!(
            matches!(
                router.list(&TransferOptions::default()),
                Err(Error::Upstream(_))
            ),
            "the upstream's error is reported"
        );
    }

    #[test]
    fn pure_swarm_lists_the_manifest_and_rejects_pushes() {
        let options = TransferOptions::default();
        let log = Log::default();
        let mut router: Router<FakeUpstream, FakeSwarm> = Router::swarm(FakeSwarm {
            log: log.clone(),
            manifest: Some(signed(A)),
            fail: false,
            publishes: false,
        });
        let refs = router.list(&options).unwrap();
        assert_eq!(names(&refs), ["HEAD", "refs/heads/main"], "manifest refs");
        let pushed = router
            .push(
                &["refs/heads/main:refs/heads/main".parse().unwrap()],
                &options,
            )
            .unwrap();
        assert!(
            matches!(pushed[0].status, PushStatus::Rejected(_)),
            "pushes need an upstream for now"
        );
    }

    #[test]
    fn upstream_push_is_mirrored_into_the_manifest() {
        let options = TransferOptions::default();
        let (mut router, log) = router(true, Some(signed(A)), false);
        router
            .push(
                &["refs/heads/main:refs/heads/main".parse().unwrap()],
                &options,
            )
            .unwrap();
        assert_eq!(
            *log.borrow(),
            [
                "upstream push",
                r#"swarm push ["refs/heads/main"] ff=false"#
            ],
            "the upstream decides; the swarm follows what it accepted"
        );
    }

    #[test]
    fn pure_swarm_push_by_the_publisher_signs_with_fast_forward_checks() {
        let log = Log::default();
        let mut router: Router<FakeUpstream, FakeSwarm> = Router::swarm(FakeSwarm {
            log: log.clone(),
            manifest: Some(signed(A)),
            fail: false,
            publishes: true,
        });
        let pushed = router
            .push(
                &["refs/heads/main:refs/heads/main".parse().unwrap()],
                &TransferOptions::default(),
            )
            .unwrap();
        assert_eq!(pushed[0].status, PushStatus::Ok);
        assert_eq!(
            *log.borrow(),
            [r#"swarm push ["refs/heads/main"] ff=true"#],
            "the manifest is the only copy, so it enforces fast-forwards"
        );
    }

    #[test]
    fn push_while_the_upstream_is_down_goes_to_the_swarm_and_is_recorded() {
        let (mut router, log) = router(false, Some(signed(A)), false);
        let pushed = router
            .push(
                &["refs/heads/main:refs/heads/main".parse().unwrap()],
                &TransferOptions::default(),
            )
            .unwrap();
        assert_eq!(pushed[0].status, PushStatus::Ok);
        assert_eq!(
            *log.borrow(),
            [
                "upstream push",
                r#"swarm push ["refs/heads/main"] ff=true"#,
                r#"swarm unsynced ["refs/heads/main"]"#
            ],
            "fast-forward checks apply, and the ref is remembered for reconcile"
        );
    }
}
