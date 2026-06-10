#[derive(Debug, Clone, Copy)]
pub struct BuiltinOssSource {
    pub tool: &'static str,
    pub platform: &'static str,
    pub arch: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub version: &'static str,
    pub official_url: &'static str,
}

// Internal OSS acceleration sources maintained by Tako Switch.
//
// Add entries only when all fields are known and the sha256 belongs to the
// exact bytes served at `url`. Entries without a checksum must not be added
// here; use manual fallback instead.
pub const BUILTIN_OSS_SOURCES: &[BuiltinOssSource] = &[];

pub(crate) fn find_builtin_oss_source_in<'a>(
    sources: &'a [BuiltinOssSource],
    tool: &str,
    platform: &str,
    arch: &str,
) -> Option<&'a BuiltinOssSource> {
    sources.iter().find(|source| {
        source.tool == tool
            && source.platform == platform
            && (source.arch == arch || source.arch == "any")
            && !source.url.trim().is_empty()
            && !source.sha256.trim().is_empty()
            && !source.official_url.trim().is_empty()
    })
}
