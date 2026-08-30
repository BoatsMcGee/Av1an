use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
};

#[inline]
pub fn hash_path(path: &Path) -> String {
    let mut s = DefaultHasher::new();
    path.hash(&mut s);
    #[expect(clippy::string_slice, reason = "we know the hash only contains ascii")]
    format!("{:x}", s.finish())[..7].to_string()
}
