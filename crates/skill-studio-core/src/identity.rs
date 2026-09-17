//! Identity model: harness ids, root references, deployment and owner ids.
//!
//! Every id is an opaque newtype around its current wire string. The `dep:v1`
//! and `owner:v1` formats stay as they are; the newtypes only stop callers
//! from building or parsing them outside the core.

use std::fmt;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};

/// Kebab-case harness identifier, for example `claude-code` or `open-code`.
///
/// Invariant: the string is the serde wire name used by the desktop app
/// today. `open-code` is canonical; `opencode` is only a CLI binary name and
/// is never stored in an `AgentId`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    /// Claude Code.
    pub const CLAUDE_CODE: &'static str = "claude-code";
    /// OpenAI Codex CLI.
    pub const CODEX: &'static str = "codex";
    /// OpenCode.
    pub const OPEN_CODE: &'static str = "open-code";
    /// pi coding agent.
    pub const PI: &'static str = "pi";
    /// Cursor.
    pub const CURSOR: &'static str = "cursor";
    /// Grok Build.
    pub const GROK_BUILD: &'static str = "grok-build";

    /// Parses a kebab-case id. Rejects empty strings and characters outside
    /// `a-z`, `0-9`, and `-`.
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let ok = !raw.is_empty()
            && raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !raw.starts_with('-')
            && !raw.ends_with('-');
        if ok {
            Ok(AgentId(raw.to_string()))
        } else {
            Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("`{raw}` is not a kebab-case harness id"),
            ))
        }
    }

    /// Returns the wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&'static str> for AgentId {
    fn from(id: &'static str) -> Self {
        AgentId(id.to_string())
    }
}

/// Lexical skill name: the directory name that holds `SKILL.md`.
///
/// Invariant: two deployments with the same `SkillName` are the same skill,
/// even when their bytes differ. A symlink alias with another name is another
/// skill.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SkillName(pub String);

impl fmt::Display for SkillName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Absolute path of a tracked project directory.
///
/// Invariant: the path is canonical inside a [`crate::scope::NormalizedScope`].
/// Wire values are the lexical path the caller gave.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ProjectRef(pub PathBuf);

/// Where a root lives.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case", tag = "scope", content = "project")]
pub enum RootScope {
    /// Under the scope home.
    Global,
    /// Under one tracked project.
    Project(ProjectRef),
}

/// What kind of root a path is.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case", tag = "kind", content = "harness")]
pub enum RootKind {
    /// A harness's own skills directory (`~/.claude/skills`).
    Harness(AgentId),
    /// The shared `.agents/skills` root.
    Universal,
    /// The `.agents/skills-parked` holding root (global only).
    Parked,
    /// A legacy directory a harness still reads (OpenCode `skill/`).
    Legacy(AgentId),
    /// A plugin cache the harness ships skills in.
    PluginCache(AgentId),
}

/// Relative path of the universal root under the home or a project.
pub const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
/// Relative path of the parked holding root under the home.
pub const PARKED_ROOT_RELATIVE: &str = ".agents/skills-parked";
/// Directory name, inside a skills root, that holds moved-aside skills.
///
/// A one-level reader never reaches `<root>/.skill-studio-disabled/<skill>`;
/// whether a recursive reader does is the harness fact
/// `skips_hidden_entries`.
pub const MOVE_ASIDE_DIR_NAME: &str = ".skill-studio-disabled";

/// A root, before it is resolved on disk.
///
/// Invariant: `(scope, kind)` is unique inside one normalized scope, and
/// `Parked` exists only at `Global`. `RootRef::new` enforces the second
/// rule; `scan` rejects a deserialized value that breaks it with
/// [`ErrorCode::InvalidRequest`] through [`RootRef::validate`].
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct RootRef {
    /// Global or project.
    pub scope: RootScope,
    /// Harness, universal, parked, legacy, or plugin cache.
    pub kind: RootKind,
}

impl RootRef {
    /// Builds a root reference, refusing `Parked` outside `Global`.
    pub fn new(scope: RootScope, kind: RootKind) -> Result<Self, CoreError> {
        let root = RootRef { scope, kind };
        root.validate()?;
        Ok(root)
    }

    /// The global parked root.
    pub fn parked() -> Self {
        RootRef {
            scope: RootScope::Global,
            kind: RootKind::Parked,
        }
    }

    /// Checks the `Parked` rule on a value built or deserialized elsewhere.
    pub fn validate(&self) -> Result<(), CoreError> {
        match (&self.scope, &self.kind) {
            (RootScope::Project(project), RootKind::Parked) => Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "the parked root exists only at global scope",
            )
            .at(&project.0)),
            _ => Ok(()),
        }
    }
}

/// A root after resolution.
///
/// Invariant: `lexical` is what the harness reads; `canonical` is where the
/// bytes live. They differ when the root is a symlink (whole-dir link).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedRoot {
    /// The root reference.
    pub root: RootRef,
    /// Path as the harness names it.
    pub lexical: PathBuf,
    /// Canonical path, or `None` when the root does not exist.
    pub canonical: Option<PathBuf>,
    /// True when `lexical` is itself a symlink.
    pub is_link: bool,
}

/// Opaque deployment id.
///
/// Invariant: the string starts with `dep:v1/`. Its internal layout is a core
/// detail; adapters and the UI compare it as a string and never split it.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct DeploymentId(String);

impl DeploymentId {
    /// Prefix every valid id carries.
    pub const PREFIX: &'static str = "dep:v1/";

    /// Accepts a wire string from a caller. Rejects ids without the prefix.
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        if raw.starts_with(Self::PREFIX) {
            Ok(DeploymentId(raw.to_string()))
        } else {
            Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("`{raw}` is not a deployment id"),
            ))
        }
    }

    /// Returns the wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque lifecycle owner id.
///
/// Invariant: the string starts with `owner:v1/`. It names the ledger entry
/// that installed the skill, not a path.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct OwnerId(String);

impl OwnerId {
    /// Prefix every valid id carries.
    pub const PREFIX: &'static str = "owner:v1/";

    /// Accepts a wire string from a caller. Rejects ids without the prefix.
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        if raw.starts_with(Self::PREFIX) {
            Ok(OwnerId(raw.to_string()))
        } else {
            Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("`{raw}` is not an owner id"),
            ))
        }
    }

    /// Returns the wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exactly one of a deployment or an owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "target", content = "id")]
pub enum LifecycleTarget {
    /// One deployment.
    Deployment(DeploymentId),
    /// Every deployment installed by one owner.
    Owner(OwnerId),
}

/// How a deployment is wired to its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackingRelationship {
    /// The directory holds the bytes.
    Canonical,
    /// A symlink to another deployment.
    LinkedTo,
    /// A copy that no longer tracks its source.
    Independent,
}

/// Whether Skill Studio may change a deployment in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMutability {
    /// Writes are allowed.
    Mutable,
    /// Plugin caches and managed ledgers: writes are refused.
    ReadOnly,
}

/// Universal (`.agents/skills`) or per-harness destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillDestination {
    /// The shared root.
    Universal,
    /// A harness's own root.
    PerHarness,
}

/// Which ledger owns a deployment's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOwnerKind {
    /// `~/.agents/.skill-lock.json`.
    SkillsSh,
    /// A dotagents ledger.
    Dotagents,
    /// A copy install recorded by Skill Studio.
    Copy,
    /// A fork recorded by Skill Studio.
    Fork,
    /// A harness plugin cache.
    Plugin,
    /// Committed in the project repository.
    InRepo,
    /// Placed by hand.
    Manual,
    /// A dotagents wildcard entry.
    WildcardDotagents,
    /// More than one ledger claims the name.
    Ambiguous,
}

impl LifecycleOwnerKind {
    /// True when Skill Studio may remove, update, or edit the deployment.
    pub const fn is_mutable(self) -> bool {
        matches!(
            self,
            LifecycleOwnerKind::SkillsSh
                | LifecycleOwnerKind::Dotagents
                | LifecycleOwnerKind::Copy
                | LifecycleOwnerKind::Fork
        )
    }
}

/// How a skill made it onto disk.
///
/// Ported from the desktop's `provenance::SourceKind`. Serializes to the same
/// kebab-case strings the frontend has always used. Declaration order doubles
/// as precedence order (dotagents beats plugin beats skills-sh beats in-repo
/// beats manual) via the derived `Ord`, matching the desktop's
/// `classify_source_kind`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// Symlinked in by getsentry/dotagents.
    Dotagents,
    /// Shipped by an agent plugin.
    Plugin,
    /// Present in the skills.sh lock file.
    SkillsSh,
    /// A plain directory with no skill-manager provenance, but sitting
    /// inside a git working tree.
    InRepo,
    /// A plain directory with no other provenance signal.
    Manual,
    /// Detached from its dotagents/skills.sh ledger via "Fork" so local
    /// edits survive `sync`/`update`. Never produced by classification
    /// directly; assigned afterward from the fork registry.
    Fork,
}

/// Content fingerprint in the `sha256:<hex>` form.
///
/// Invariant: the hex is lowercase and 64 characters long. One scheme covers
/// files, trees, and proposals; the old untyped hex form is parsed on read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Prefix of the typed form.
    pub const PREFIX: &'static str = "sha256:";

    /// Hashes bytes.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Fingerprint(format!("{}{}", Self::PREFIX, sha256_hex(bytes)))
    }

    /// Accepts `sha256:<hex>` or a bare 64-character hex (legacy rows).
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let hex = raw.strip_prefix(Self::PREFIX).unwrap_or(raw);
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Fingerprint(format!(
                "{}{}",
                Self::PREFIX,
                hex.to_ascii_lowercase()
            )))
        } else {
            Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("`{raw}` is not a sha256 fingerprint"),
            ))
        }
    }

    /// Returns the typed wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the bare hex, without the `sha256:` prefix - the form the
    /// desktop's `InverseOp` schema stores for `pre_fingerprint`/
    /// `post_fingerprint` (alongside its literal `"absent"` sentinel), so
    /// any code writing or comparing against that schema must use this, not
    /// [`Fingerprint::as_str`], or a core-authored row would never drift-
    /// check clean against a desktop-authored one and vice versa.
    pub fn bare_hex(&self) -> &str {
        self.0.strip_prefix(Self::PREFIX).unwrap_or(&self.0)
    }
}

/// Id of a frontmatter repair proposal.
///
/// Invariant: sha256 over deployment id, path, owner id, owner kind, the
/// expected fingerprint, and the proposed text. Any change makes a new id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ProposalId(pub String);

/// History event id (ULID string).
///
/// Invariant: lexical order equals creation order inside one store.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct EventId(pub String);

impl EventId {
    /// Wraps a freshly generated ULID.
    pub fn from_ulid(id: ulid::Ulid) -> Self {
        EventId(id.to_string())
    }
}

/// Correlation id an adapter attaches to one request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CorrelationId(pub String);

/// Returns the lexical last path component, or an empty string.
pub fn leaf_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// SHA-256 as lowercase hex.
///
/// Implemented here so the core needs no crypto dependency. The digest must
/// equal `shasum -a 256`; see the test below.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn ids_keep_their_v1_prefixes() {
        assert!(DeploymentId::parse("dep:v1/global/codex/per-harness/x/-/p").is_ok());
        assert!(DeploymentId::parse("dep:v2/x").is_err());
        assert!(OwnerId::parse("owner:v1/global/x").is_ok());
        assert!(AgentId::parse("open-code").is_ok());
        assert!(AgentId::parse("OpenCode").is_err());
    }

    #[test]
    fn parked_root_exists_only_at_global() {
        assert!(RootRef::new(RootScope::Global, RootKind::Parked).is_ok());
        let project = RootScope::Project(ProjectRef(PathBuf::from("/home/u/src/app")));
        let err = RootRef::new(project.clone(), RootKind::Parked).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(RootRef::new(project, RootKind::Universal).is_ok());
        assert_eq!(RootRef::parked().validate().ok(), Some(()));
    }

    #[test]
    fn fingerprint_accepts_legacy_bare_hex() {
        let typed = Fingerprint::of_bytes(b"abc");
        let bare = Fingerprint::parse(&typed.as_str()[Fingerprint::PREFIX.len()..]).unwrap();
        assert_eq!(typed, bare);
    }
}
