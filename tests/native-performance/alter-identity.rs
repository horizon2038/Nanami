//! Production cache and Linux getter adapter; POSIX transport is modeled.
extern crate self as nanami_client;
extern crate self as nanami_runtime;

pub type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Io,
}

#[path = "../../nanami/servers/apps/alter/shared/src/common/state/identity_cache.rs"]
mod identity_cache;

pub mod state {
    pub use super::identity_cache::{IdentityCache, IdentityField};
    use super::Word;

    pub struct Runtime {
        pub posix_port: Word,
        pub identity_cache: IdentityCache,
    }
    impl Runtime {
        pub fn new(port: Word) -> Self {
            Self {
                posix_port: port,
                identity_cache: IdentityCache::new(),
            }
        }
    }
}

pub mod posix {
    use super::{RequestError, Word};
    use std::cell::RefCell;
    #[derive(Default)]
    pub struct Mock {
        pub calls: Vec<(Word, usize)>,
        pub failures: usize,
        pub generation: Word,
    }
    thread_local! {
        pub static MOCK: RefCell<Mock> = RefCell::new(Mock::default());
    }
    fn query(port: Word, field: usize) -> Result<Word, RequestError> {
        MOCK.with(|mock| {
            let mut mock = mock.borrow_mut();
            mock.calls.push((port, field));
            if mock.failures != 0 {
                mock.failures -= 1;
                return Err(RequestError::Io);
            }
            Ok(port * 100 + field + mock.generation)
        })
    }
    pub fn posix_getuid(port: Word) -> Result<Word, RequestError> {
        query(port, 0)
    }
    pub fn posix_geteuid(port: Word) -> Result<Word, RequestError> {
        query(port, 1)
    }
    pub fn posix_getgid(port: Word) -> Result<Word, RequestError> {
        query(port, 2)
    }
    pub fn posix_getegid(port: Word) -> Result<Word, RequestError> {
        query(port, 3)
    }
}

#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/identity.rs"]
mod identity;
use state::{IdentityCache, IdentityField, Runtime};

fn fields() -> [IdentityField; 4] {
    [
        IdentityField::RealUser,
        IdentityField::EffectiveUser,
        IdentityField::RealGroup,
        IdentityField::EffectiveGroup,
    ]
}

#[test]
fn cold_fields_query_their_exact_identity_once_and_warm_reads_are_local() {
    let mut runtime = Runtime::new(7);
    for _ in 0..10_000 {
        for (index, field) in fields().into_iter().enumerate() {
            assert_eq!(identity::read(&mut runtime, field), Ok(700 + index));
        }
    }
    posix::MOCK.with(|mock| assert_eq!(mock.borrow().calls, [(7, 0), (7, 1), (7, 2), (7, 3)]));
}

#[test]
fn a_failed_field_is_retried_without_discarding_other_valid_fields() {
    let mut runtime = Runtime::new(4);
    assert_eq!(
        identity::read(&mut runtime, IdentityField::RealUser),
        Ok(400)
    );
    posix::MOCK.with(|mock| mock.borrow_mut().failures = 1);
    assert_eq!(
        identity::read(&mut runtime, IdentityField::EffectiveUser),
        Err(RequestError::Io)
    );
    assert_eq!(
        identity::read(&mut runtime, IdentityField::RealUser),
        Ok(400)
    );
    assert_eq!(
        identity::read(&mut runtime, IdentityField::EffectiveUser),
        Ok(401)
    );
    assert_eq!(
        identity::read(&mut runtime, IdentityField::EffectiveUser),
        Ok(401)
    );
    posix::MOCK.with(|mock| assert_eq!(mock.borrow().calls, [(4, 0), (4, 1), (4, 1)]));
}

#[test]
fn a_different_connection_invalidates_every_cached_field() {
    let mut runtime = Runtime::new(3);
    for field in fields() {
        identity::read(&mut runtime, field).unwrap();
    }
    runtime.posix_port = 9;
    for (index, field) in fields().into_iter().enumerate() {
        assert_eq!(identity::read(&mut runtime, field), Ok(900 + index));
    }
    posix::MOCK.with(|mock| assert_eq!(mock.borrow().calls.len(), 8));
}

#[test]
fn same_descriptor_rebinding_explicitly_clears_the_old_session_snapshot() {
    let mut runtime = Runtime::new(3);
    assert_eq!(
        identity::read(&mut runtime, IdentityField::RealUser),
        Ok(300)
    );
    posix::MOCK.with(|mock| mock.borrow_mut().generation = 40);
    runtime.identity_cache.clear();
    assert_eq!(
        identity::read(&mut runtime, IdentityField::RealUser),
        Ok(340)
    );
    posix::MOCK.with(|mock| assert_eq!(mock.borrow().calls.len(), 2));
}

#[test]
fn independent_runtimes_do_not_share_cached_identity_state() {
    let mut a = Runtime::new(8);
    let mut b = Runtime::new(8);
    identity::read(&mut a, IdentityField::RealUser).unwrap();
    identity::read(&mut b, IdentityField::RealUser).unwrap();
    posix::MOCK.with(|mock| assert_eq!(mock.borrow().calls, [(8, 0), (8, 0)]));
}

#[test]
fn zero_identity_is_a_value_not_an_empty_cache_sentinel() {
    let mut cache = IdentityCache::new();
    assert_eq!(
        cache.get_or_query::<()>(1, IdentityField::RealUser, || Ok(0)),
        Ok(0)
    );
    assert_eq!(
        cache.get_or_query::<()>(1, IdentityField::RealUser, || panic!("warm query")),
        Ok(0)
    );
    assert!(std::mem::size_of::<IdentityCache>() <= 6 * std::mem::size_of::<Word>());
}
