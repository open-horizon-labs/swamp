//! The workspace as the compiler sees it, for path-reference rules.
//!
//! This is deliberately *not* a call-graph analysis. Four review rounds
//! showed that a `syn` model without type resolution cannot decide what a
//! call reaches (fn pointers, UFCS, generics, macros, cfg, globs). The
//! guardrail semantics moved into types (`crates/core/src/fs_gate`); what
//! is left for this model is what token analysis does *exactly*:
//!
//! * **The module tree**, from each crate root's `mod` declarations
//!   (`#[path]` recorded, orphans reported): a file no `mod` reaches is
//!   not part of the crate.
//! * **Test code by attribute**: an item is test code when an attribute
//!   `#[cfg(..)]` whose predicate requires `test` (or the `testing`
//!   feature) is on it or an ancestor, or it is a `#[test]` fn. Doc
//!   comments are `#[doc]` attributes and never count.
//! * **Every path reference, resolved**: in expressions, types,
//!   patterns, bounds, qualified-self types, `use` trees (renames, globs),
//!   struct literals, and the token streams of macro invocations *and*
//!   `macro_rules!` definitions -- position-independent, so a fn item
//!   bound to a local, stored in a field or a `const`, or passed as an
//!   argument is a reference like any call.
//! * **Method-call names, string literals, `unsafe`/`extern`/`#[path]`
//!   sites, and serialized field names**, each with its test/production
//!   context and location.
//!
//! The one rule that still follows calls, `tui_event_thread_has_no_gate_calls`,
//! uses [`FnDef`] conservatively: an unresolvable callee in the region
//! is a rejection, and a method call matches every workspace method of
//! that name.

use proc_macro2::{TokenStream, TokenTree};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// A workspace crate the audits cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Krate {
    Core,
    Cli,
    Tui,
}

impl Krate {
    pub const ALL: [Krate; 3] = [Krate::Core, Krate::Cli, Krate::Tui];

    /// The first segment of an absolute path into this crate.
    pub fn root_segment(self) -> &'static str {
        match self {
            Krate::Core => "@core",
            Krate::Cli => "@cli",
            Krate::Tui => "@tui",
        }
    }

    pub fn dir(self) -> &'static str {
        match self {
            Krate::Core => "crates/core",
            Krate::Cli => "crates/cli",
            Krate::Tui => "crates/tui",
        }
    }

    fn root_file(self) -> &'static str {
        match self {
            Krate::Core => "crates/core/src/lib.rs",
            Krate::Cli => "crates/cli/src/main.rs",
            Krate::Tui => "crates/tui/src/lib.rs",
        }
    }

    /// Extern crate names that mean a workspace crate.
    fn extern_name(name: &str) -> Option<Krate> {
        match name {
            "swamp_core" => Some(Krate::Core),
            "swamp_tui" => Some(Krate::Tui),
            _ => None,
        }
    }
}

/// Where something was found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    pub file: String,
    pub line: usize,
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A `use` tree leaf (`use a::b;`, `use a::b as c;`).
    Use,
    /// A `use a::b::*;` glob: the referenced path is `a::b`.
    Glob,
    /// Anywhere else: expression, type, pattern, bound, macro tokens.
    Code,
}

/// One path as written, with its context.
#[derive(Debug, Clone)]
pub struct Ref {
    pub segments: Vec<String>,
    pub kind: RefKind,
    pub test: bool,
    pub site: Site,
    /// Index into [`Workspace::fns`] of the enclosing function, if any.
    pub in_fn: Option<usize>,
    /// Inside the arguments of a `worker::spawn(..)` call: work the TUI
    /// hands off its event thread.
    pub in_worker: bool,
    /// Inside an `impl` of the very type this path's last segment names
    /// (`impl Consumer for X`, `X::new()` inside `impl X`): a type naming
    /// itself does not make it used.
    pub own_impl: bool,
}

#[derive(Debug, Clone)]
pub struct MethodCall {
    pub name: String,
    pub test: bool,
    pub site: Site,
    pub in_fn: Option<usize>,
    pub in_worker: bool,
}

#[derive(Debug, Clone)]
pub struct Literal {
    pub value: String,
    pub test: bool,
    pub site: Site,
}

#[derive(Debug, Clone)]
pub struct MacroCall {
    /// The macro's last path segment (`println`, `panic`, `format`).
    pub name: String,
    pub test: bool,
    pub site: Site,
    pub in_fn: Option<usize>,
}

/// A place the compiler would accept only with the gate's allowances.
#[derive(Debug, Clone)]
pub struct Hazard {
    pub what: String,
    pub test: bool,
    pub site: Site,
}

/// A field (or enum variant) name that `serde` serializes as a key.
#[derive(Debug, Clone)]
pub struct SerializedName {
    pub owner: String,
    pub name: String,
    pub test: bool,
    pub site: Site,
}

/// One `use` binding: `alias` in this module means `target`.
#[derive(Debug, Clone)]
pub struct UseBinding {
    pub alias: String,
    pub target: Vec<String>,
}

/// One function or method body.
#[derive(Debug, Clone)]
pub struct FnDef {
    pub module: usize,
    pub name: String,
    /// The `impl` self type's last segment, or the trait name for a
    /// trait's default method.
    pub owner: Option<String>,
    pub test: bool,
    /// Declared `pub` (exactly: `pub(crate)` items are rustc's
    /// `dead_code` lint's business).
    pub public: bool,
    /// A method of an `impl Trait for Type` (dispatched through the
    /// trait, never named).
    pub trait_impl: bool,
    /// The trait's last segment, for a method of `impl Trait for Type`.
    pub impl_trait: Option<String>,
    pub site: Site,
    /// Calls whose callee is not a path (`(self.f)(x)`, `t[0](x)`,
    /// `g()(x)`): what the conservative TUI rule rejects.
    pub opaque_calls: Vec<Site>,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub krate: Krate,
    /// Module path below the crate root (`["fs_gate", "destroy"]`).
    pub path: Vec<String>,
    pub file: String,
    pub test: bool,
    pub uses: Vec<UseBinding>,
    pub globs: Vec<Vec<String>>,
    /// Names of items and child modules defined directly here.
    pub items: HashSet<String>,
    pub refs: Vec<Ref>,
    pub methods: Vec<MethodCall>,
    pub literals: Vec<Literal>,
    pub macros: Vec<MacroCall>,
    pub hazards: Vec<Hazard>,
    pub serialized: Vec<SerializedName>,
    /// `pub` types, traits, consts and statics defined here (production).
    pub pub_items: Vec<(String, Site)>,
    /// Module-level `const NAME: &str = "value";` items.
    pub str_consts: Vec<(String, String, Site)>,
    /// Absolute or home-relative path literals turned into a path
    /// (`PathBuf::from("/Users/..")`, `Path::new("~/.x")`), production only.
    pub path_literals: Vec<(String, Site)>,
    /// `#[allow(dead_code)]` / `#[expect(dead_code)]` in production code.
    pub dead_code_allows: Vec<Site>,
    /// A lint level lowered on one of the capability gate's lints
    /// (`clippy::disallowed_methods`/`disallowed_types`/`disallowed_macros`,
    /// the `clippy::style`/`clippy::all` groups that contain them,
    /// `unsafe_code`, `warnings`): `(what, site, test)`.
    pub lint_allows: Vec<(String, Site, bool)>,
    /// Every `include!`/`include_str!`/`include_bytes!` in this module.
    pub includes: Vec<Include>,
    /// Every struct literal (`Plan { .. }`), with `Self` resolved to the
    /// enclosing `impl`'s type.
    pub struct_literals: Vec<StructLiteral>,
}

/// One struct-literal expression.
#[derive(Debug, Clone)]
pub struct StructLiteral {
    pub segments: Vec<String>,
    pub test: bool,
    pub site: Site,
    pub in_fn: Option<usize>,
}

/// One `include!`-family macro invocation.
#[derive(Debug, Clone)]
pub struct Include {
    /// `include`, `include_str` or `include_bytes`.
    pub kind: String,
    /// The target, workspace-relative, when the argument is one string
    /// literal; `None` for a computed path (`concat!(env!(..), ..)`).
    pub target: Option<String>,
    pub test: bool,
    pub site: Site,
}

impl Module {
    /// `@core::fs_gate::destroy`, the module's own absolute path.
    pub fn abs(&self) -> Vec<String> {
        let mut v = vec![self.krate.root_segment().to_string()];
        v.extend(self.path.iter().cloned());
        v
    }

    pub fn display(&self) -> String {
        format!("{}::{}", self.krate.root_segment(), self.path.join("::"))
    }

    /// Whether this module is `path` or lies beneath it.
    pub fn is_within(&self, krate: Krate, path: &[&str]) -> bool {
        self.krate == krate
            && self.path.len() >= path.len()
            && self.path.iter().zip(path).all(|(a, b)| a == b)
    }
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub root: PathBuf,
    pub modules: Vec<Module>,
    pub fns: Vec<FnDef>,
    /// `.rs` files under a crate's `src/` that no `mod` declaration
    /// reaches.
    pub orphans: Vec<String>,
    /// Files that do not parse (the rule that reads them fails loudly).
    pub parse_errors: Vec<String>,
    /// Build scripts found in workspace crates the audits do not model
    /// (`crates/harvest`, `crates/source-audit`), workspace-relative.
    pub unmodelled_build_scripts: Vec<String>,
    /// Files spliced in by `include!` (so they are not orphans).
    pub included: HashSet<String>,
}

impl Workspace {
    pub fn load(root: &Path) -> Workspace {
        let mut ws = Workspace {
            root: root.to_path_buf(),
            ..Default::default()
        };
        for krate in Krate::ALL {
            let mut reached: HashSet<String> = HashSet::new();
            let root_file = krate.root_file().to_string();
            ws.load_file(krate, Vec::new(), &root_file, false, &mut reached);
            // A build script is code Cargo compiles and *runs* on every
            // build: it is loaded as a module of its crate (under a path
            // segment no `mod` can spell) so every rule sees what it
            // names.
            for script in build_scripts(root, krate.dir()) {
                ws.load_file(
                    krate,
                    vec![BUILD_SCRIPT_SEGMENT.to_string()],
                    &script,
                    false,
                    &mut reached,
                );
            }
            let src = root.join(krate.dir()).join("src");
            for f in rust_files(&src) {
                let rel = rel(root, &f);
                if !reached.contains(&rel) && !ws.included.contains(&rel) {
                    ws.orphans.push(rel);
                }
            }
        }
        // The other workspace members (developer tools) are not modelled;
        // a build script there would run unaudited on `cargo build
        // --workspace`, so the gate rule rejects any.
        if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                let dir = rel(root, &d);
                if Krate::ALL.iter().any(|k| k.dir() == dir) {
                    continue;
                }
                ws.unmodelled_build_scripts
                    .extend(build_scripts(root, &dir));
            }
        }
        ws.orphans.sort();
        ws
    }

    fn load_file(
        &mut self,
        krate: Krate,
        path: Vec<String>,
        rel_file: &str,
        test: bool,
        reached: &mut HashSet<String>,
    ) {
        if !reached.insert(rel_file.to_string()) {
            return;
        }
        let text = match std::fs::read_to_string(self.root.join(rel_file)) {
            Ok(t) => t,
            Err(e) => {
                self.parse_errors.push(format!("{rel_file}: {e}"));
                return;
            }
        };
        let file = match parse_cached(&text) {
            Ok(f) => f,
            Err(e) => {
                self.parse_errors.push(format!("{rel_file}: {e}"));
                return;
            }
        };
        let test = test || attrs_test(&file.attrs);
        let dir = child_dir(rel_file);
        self.load_items(
            krate,
            path,
            rel_file,
            &dir,
            test,
            &file.attrs,
            &file.items,
            reached,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn load_items(
        &mut self,
        krate: Krate,
        path: Vec<String>,
        rel_file: &str,
        child_dir: &str,
        test: bool,
        inner_attrs: &[syn::Attribute],
        items: &[syn::Item],
        reached: &mut HashSet<String>,
    ) {
        let idx = self.modules.len();
        self.modules.push(Module {
            krate,
            path: path.clone(),
            file: rel_file.to_string(),
            test,
            uses: Vec::new(),
            globs: Vec::new(),
            items: HashSet::new(),
            refs: Vec::new(),
            methods: Vec::new(),
            literals: Vec::new(),
            macros: Vec::new(),
            hazards: Vec::new(),
            serialized: Vec::new(),
            pub_items: Vec::new(),
            str_consts: Vec::new(),
            path_literals: Vec::new(),
            dead_code_allows: Vec::new(),
            lint_allows: Vec::new(),
            includes: Vec::new(),
            struct_literals: Vec::new(),
        });
        // Item names first, so resolution can see later items.
        for item in items {
            if let Some(name) = item_name(item) {
                self.modules[idx].items.insert(name);
            }
        }
        {
            let mut c = Collector {
                ws: self,
                module: idx,
                test_depth: usize::from(test),
                fn_stack: Vec::new(),
                impl_owner: None,
                in_trait_impl: false,
                in_trait_def: false,
                impl_trait: None,
                worker_depth: 0,
                spawn_aliases: HashSet::new(),
            };
            for a in inner_attrs {
                c.visit_attribute(a);
            }
            for item in items {
                if let syn::Item::Mod(_) = item {
                    // Child modules are loaded separately below; their
                    // own attributes are still checked here.
                    if let syn::Item::Mod(m) = item {
                        for a in &m.attrs {
                            c.visit_attribute(a);
                        }
                    }
                    continue;
                }
                c.visit_item(item);
            }
        }
        for item in items {
            let syn::Item::Mod(m) = item else { continue };
            let name = m.ident.to_string();
            let child_test = test || attrs_test(&m.attrs);
            let mut child_path = path.clone();
            child_path.push(name.clone());
            let explicit = path_attr(&m.attrs);
            if explicit.is_some() {
                self.modules[idx].hazards.push(Hazard {
                    what: format!("`#[path]` on `mod {name}`"),
                    test: child_test,
                    site: Site {
                        file: rel_file.to_string(),
                        line: m.span().start().line,
                    },
                });
            }
            match &m.content {
                Some((_, inner)) => {
                    let dir = format!("{child_dir}/{name}");
                    self.load_items(
                        krate, child_path, rel_file, &dir, child_test, &m.attrs, inner, reached,
                    );
                }
                None => {
                    let candidates: Vec<String> = match &explicit {
                        Some(p) => vec![format!("{}/{p}", parent_dir(rel_file))],
                        None => vec![
                            format!("{child_dir}/{name}.rs"),
                            format!("{child_dir}/{name}/mod.rs"),
                        ],
                    };
                    let found = candidates.into_iter().find(|c| self.root.join(c).is_file());
                    match found {
                        Some(f) => self.load_file(krate, child_path, &f, child_test, reached),
                        None => self.parse_errors.push(format!(
                            "{rel_file}: `mod {name};` has no file (tried {child_dir}/{name}.rs, \
                             {child_dir}/{name}/mod.rs)"
                        )),
                    }
                }
            }
        }
    }

    /// Resolves `segments` as written in module `m` to an absolute path:
    /// `@core::…`/`@cli::…`/`@tui::…` for workspace items, `std::…`,
    /// `libc::…` etc. for everything else. A first segment that names
    /// nothing in scope (a local, a generic, a prelude type) comes back
    /// prefixed with `?`.
    pub fn resolve(&self, m: usize, segments: &[String]) -> Vec<String> {
        self.resolve_depth(m, segments, 0)
    }

    fn resolve_depth(&self, m: usize, segments: &[String], depth: usize) -> Vec<String> {
        let module = &self.modules[m];
        if segments.is_empty() || depth > 16 {
            return segments.to_vec();
        }
        let first = segments[0].as_str();
        let rest = &segments[1..];
        let join = |mut base: Vec<String>, rest: &[String]| {
            base.extend(rest.iter().cloned());
            base
        };
        match first {
            "crate" => join(vec![module.krate.root_segment().to_string()], rest),
            "self" => join(module.abs(), rest),
            "super" => {
                let mut base = module.abs();
                let mut rest = rest;
                base.pop();
                while rest.first().is_some_and(|s| s == "super") {
                    base.pop();
                    rest = &rest[1..];
                }
                join(base, rest)
            }
            "Self" => segments.to_vec(),
            _ => {
                if let Some(k) = Krate::extern_name(first) {
                    return join(vec![k.root_segment().to_string()], rest);
                }
                if let Some(u) = module.uses.iter().rev().find(|u| u.alias == first) {
                    let target = self.resolve_depth(m, &u.target, depth + 1);
                    return join(target, rest);
                }
                if module.items.contains(first) {
                    return join(module.abs(), segments);
                }
                if is_extern_crate(first) {
                    return segments.to_vec();
                }
                let mut v = vec![format!("?{first}")];
                v.extend(rest.iter().cloned());
                v
            }
        }
    }

    /// Every production `pub` type/trait/const/static in the workspace.
    pub fn public_types(&self) -> Vec<(String, Site)> {
        self.modules
            .iter()
            .filter(|m| !m.test)
            .flat_map(|m| m.pub_items.iter().cloned())
            .collect()
    }

    /// Every module whose path is exactly `abs` (`@core::fs_gate`).
    pub fn module_at(&self, abs: &[String]) -> Option<&Module> {
        self.modules.iter().find(|m| m.abs() == abs)
    }
}

/// The module path segment a build script is loaded under
/// (`@core::(build.rs)`): not an identifier, so no `mod` reaches it and
/// no path in the crate can name it.
pub const BUILD_SCRIPT_SEGMENT: &str = "(build.rs)";

/// The build scripts of the crate at `crate_dir` (workspace-relative):
/// `build.rs` next to its manifest, or whatever `package.build` names.
/// `build = false` disables it.
pub fn build_scripts(root: &Path, crate_dir: &str) -> Vec<String> {
    let manifest =
        std::fs::read_to_string(root.join(crate_dir).join("Cargo.toml")).unwrap_or_default();
    let mut explicit: Option<Option<String>> = None;
    let mut section = String::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            continue;
        }
        if section == "[package]"
            && let Some(v) = t.strip_prefix("build")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            let v = v.trim();
            explicit = Some(if v == "false" {
                None
            } else {
                Some(v.trim_matches('"').to_string())
            });
        }
    }
    let file = match explicit {
        Some(None) => return Vec::new(),
        Some(Some(f)) => f,
        None => "build.rs".to_string(),
    };
    let rel_file = format!("{crate_dir}/{file}");
    if root.join(&rel_file).is_file() {
        vec![rel_file]
    } else {
        Vec::new()
    }
}

/// Lints the capability gate relies on, and the groups that contain them:
/// lowering any of these outside the gate modules re-opens what the crate
/// root denies.
const GATE_LINTS: &[&str] = &[
    "disallowed_methods",
    "disallowed_types",
    "disallowed_macros",
    "style",
    "all",
    "unsafe_code",
    "warnings",
];

/// Whether an attribute's token stream lowers a [`GATE_LINTS`] lint:
/// `allow(..)`, `expect(..)` or `warn(..)` -- directly, or inside a
/// `cfg_attr(.., allow(..))`.
fn lowers_gate_lint(ts: TokenStream) -> Option<String> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Ident(id)
                if matches!(id.to_string().as_str(), "allow" | "expect" | "warn") =>
            {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    let mut words = HashSet::new();
                    collect_idents(g.stream(), &mut words);
                    if let Some(w) = GATE_LINTS.iter().find(|l| words.contains(**l)) {
                        return Some(format!("`{id}({w})`"));
                    }
                }
            }
            TokenTree::Group(g) => {
                if let Some(hit) = lowers_gate_lint(g.stream()) {
                    return Some(hit);
                }
            }
            _ => {}
        }
    }
    None
}

/// Crates that are not in the workspace but are real first segments.
fn is_extern_crate(name: &str) -> bool {
    name.starts_with("gix_")
        || matches!(
            name,
            "std"
                | "core"
                | "alloc"
                | "libc"
                | "tempfile"
                | "trash"
                | "walkdir"
                | "jwalk"
                | "tokio"
                | "parquet"
                | "arrow_array"
                | "arrow_schema"
                | "serde"
                | "serde_json"
                | "anyhow"
                | "blake3"
                | "zstd"
                | "gix"
                | "toml"
                | "uuid"
                | "roxmltree"
                | "fsevent_sys"
                | "core_foundation"
                | "core_foundation_sys"
                | "futures_util"
                | "async_trait"
                | "ratatui"
                | "crossterm"
                | "clap"
                | "unicode_width"
                | "unicode_segmentation"
        )
}

fn item_name(item: &syn::Item) -> Option<String> {
    Some(match item {
        syn::Item::Const(i) => i.ident.to_string(),
        syn::Item::Enum(i) => i.ident.to_string(),
        syn::Item::Fn(i) => i.sig.ident.to_string(),
        syn::Item::Mod(i) => i.ident.to_string(),
        syn::Item::Static(i) => i.ident.to_string(),
        syn::Item::Struct(i) => i.ident.to_string(),
        syn::Item::Trait(i) => i.ident.to_string(),
        syn::Item::Type(i) => i.ident.to_string(),
        syn::Item::Union(i) => i.ident.to_string(),
        syn::Item::Macro(i) => i.ident.as_ref()?.to_string(),
        _ => return None,
    })
}

/// `crates/core/src/lib.rs` → `crates/core/src`; `…/growth.rs` →
/// `…/growth`; `…/agents/mod.rs` → `…/agents`.
fn child_dir(rel_file: &str) -> String {
    let p = Path::new(rel_file);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parent = parent_dir(rel_file);
    if matches!(stem, "lib" | "main" | "mod") {
        parent
    } else {
        format!("{parent}/{stem}")
    }
}

/// `a/b/../c/./d.rs` -> `a/c/d.rs`, lexically (a `..` past the start is
/// kept, so a path leaving the workspace stays visibly outside it).
pub fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|s| *s != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn parent_dir(rel_file: &str) -> String {
    Path::new(rel_file)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    for a in attrs {
        if a.path().is_ident("path")
            && let syn::Meta::NameValue(nv) = &a.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            return Some(s.value());
        }
        if a.path().is_ident("cfg_attr") {
            let text = a.meta.to_token_stream_string();
            if text.contains("path") {
                return Some(String::from("<cfg_attr path>"));
            }
        }
    }
    None
}

trait MetaText {
    fn to_token_stream_string(&self) -> String;
}

impl MetaText for syn::Meta {
    fn to_token_stream_string(&self) -> String {
        quote::ToTokens::to_token_stream(self).to_string()
    }
}

// ---------------------------------------------------------------------
// cfg evaluation
// ---------------------------------------------------------------------

/// Three-valued `cfg` predicate: `test` and the `testing` feature are
/// false for production; any other feature, platform or flag is
/// unknown (the item may be compiled somewhere, so it counts).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tri {
    True,
    False,
    Maybe,
}

fn eval_cfg(meta: &syn::Meta) -> Tri {
    match meta {
        syn::Meta::Path(p) => {
            if p.is_ident("test") {
                Tri::False
            } else {
                Tri::Maybe
            }
        }
        syn::Meta::NameValue(nv) => {
            if nv.path.is_ident("feature")
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                && s.value() == "testing"
            {
                return Tri::False;
            }
            Tri::Maybe
        }
        syn::Meta::List(l) => {
            let Ok(args) = l.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return Tri::Maybe;
            };
            let vals: Vec<Tri> = args.iter().map(eval_cfg).collect();
            if l.path.is_ident("any") {
                if vals.contains(&Tri::True) {
                    Tri::True
                } else if vals.iter().all(|v| *v == Tri::False) {
                    Tri::False
                } else {
                    Tri::Maybe
                }
            } else if l.path.is_ident("all") {
                if vals.contains(&Tri::False) {
                    Tri::False
                } else if vals.iter().all(|v| *v == Tri::True) {
                    Tri::True
                } else {
                    Tri::Maybe
                }
            } else if l.path.is_ident("not") {
                match vals.as_slice() {
                    [Tri::True] => Tri::False,
                    [Tri::False] => Tri::True,
                    _ => Tri::Maybe,
                }
            } else {
                Tri::Maybe
            }
        }
    }
}

/// Whether these attributes make an item test-only (never in a
/// production build): a `#[cfg(p)]` whose `p` is false without `test`
/// and the `testing` feature, or `#[test]`. `#[cfg(any())]` (never
/// compiled at all) is *not* test code: it stays visible, so the gate
/// rules see what it names.
pub fn attrs_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if a.path().is_ident("test") {
            return true;
        }
        if !a.path().is_ident("cfg") {
            return false;
        }
        let syn::Meta::List(l) = &a.meta else {
            return false;
        };
        let Ok(inner) = l.parse_args::<syn::Meta>() else {
            return false;
        };
        eval_cfg(&inner) == Tri::False && mentions_test(&inner)
    })
}

fn mentions_test(meta: &syn::Meta) -> bool {
    let text = quote::ToTokens::to_token_stream(meta).to_string();
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|t| t == "test" || t == "testing")
}

/// Whether an attribute list marks a test ignored, in any spelling
/// (`#[ignore]`, `#[ignore = ".."]`, `#[cfg_attr(.., ignore)]`).
pub fn attrs_ignored(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if a.path().is_ident("ignore") {
            return true;
        }
        if a.path().is_ident("cfg_attr") {
            let text = quote::ToTokens::to_token_stream(&a.meta).to_string();
            return text
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|t| t == "ignore");
        }
        false
    })
}

// ---------------------------------------------------------------------
// the collector
// ---------------------------------------------------------------------

struct Collector<'w> {
    ws: &'w mut Workspace,
    module: usize,
    test_depth: usize,
    fn_stack: Vec<usize>,
    impl_owner: Option<String>,
    in_trait_impl: bool,
    in_trait_def: bool,
    impl_trait: Option<String>,
    worker_depth: usize,
    /// Locals bound to `worker::spawn` itself (`let go = worker::spawn;`):
    /// calling one is the same hand-off.
    spawn_aliases: HashSet<String>,
}

impl Collector<'_> {
    fn test(&self) -> bool {
        self.test_depth > 0
    }

    fn site(&self, span: proc_macro2::Span) -> Site {
        Site {
            file: self.ws.modules[self.module].file.clone(),
            line: span.start().line,
        }
    }

    fn in_fn(&self) -> Option<usize> {
        self.fn_stack.last().copied()
    }

    fn push_ref(&mut self, segments: Vec<String>, kind: RefKind, span: proc_macro2::Span) {
        if segments.is_empty() {
            return;
        }
        let r = Ref {
            kind,
            test: self.test(),
            site: self.site(span),
            in_fn: self.in_fn(),
            in_worker: self.worker_depth > 0,
            own_impl: self.impl_owner.is_some()
                && !self.in_trait_def
                && segments
                    .iter()
                    .any(|s| s == "Self" || Some(s) == self.impl_owner.as_ref()),
            segments,
        };
        self.ws.modules[self.module].refs.push(r);
    }

    fn hazard(&mut self, what: impl Into<String>, span: proc_macro2::Span) {
        let h = Hazard {
            what: what.into(),
            test: self.test(),
            site: self.site(span),
        };
        self.ws.modules[self.module].hazards.push(h);
    }

    fn with_attrs<F: FnOnce(&mut Self)>(&mut self, attrs: &[syn::Attribute], f: F) {
        let t = attrs_test(attrs);
        if t {
            self.test_depth += 1;
        }
        for a in attrs {
            self.visit_attribute(a);
        }
        f(self);
        if t {
            self.test_depth -= 1;
        }
    }

    fn begin_fn(&mut self, name: String, public: bool, span: proc_macro2::Span) {
        let def = FnDef {
            module: self.module,
            name,
            owner: self.impl_owner.clone(),
            test: self.test(),
            public: public && !self.in_trait_def,
            trait_impl: self.in_trait_impl || self.in_trait_def,
            impl_trait: self.impl_trait.clone(),
            site: self.site(span),
            opaque_calls: Vec::new(),
        };
        self.ws.fns.push(def);
        self.fn_stack.push(self.ws.fns.len() - 1);
    }

    fn use_tree(&mut self, tree: &syn::UseTree, prefix: Vec<String>, span: proc_macro2::Span) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut pre = prefix;
                pre.push(p.ident.to_string());
                self.use_tree(&p.tree, pre, span);
            }
            syn::UseTree::Name(n) => {
                let name = n.ident.to_string();
                let mut target = prefix.clone();
                let alias = if name == "self" {
                    prefix.last().cloned().unwrap_or_default()
                } else {
                    target.push(name.clone());
                    name
                };
                self.push_ref(target.clone(), RefKind::Use, span);
                self.ws.modules[self.module]
                    .uses
                    .push(UseBinding { alias, target });
            }
            syn::UseTree::Rename(r) => {
                let mut target = prefix;
                if r.ident != "self" {
                    target.push(r.ident.to_string());
                }
                self.push_ref(target.clone(), RefKind::Use, span);
                self.ws.modules[self.module].uses.push(UseBinding {
                    alias: r.rename.to_string(),
                    target,
                });
            }
            syn::UseTree::Glob(_) => {
                self.push_ref(prefix.clone(), RefKind::Glob, span);
                self.ws.modules[self.module].globs.push(prefix);
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    self.use_tree(t, prefix.clone(), span);
                }
            }
        }
    }

    /// Paths, method calls, literals and `unsafe` inside a token stream
    /// that did not parse as expressions (a `macro_rules!` body, an
    /// unknown macro's arguments).
    fn scan_tokens(&mut self, ts: TokenStream) {
        let toks: Vec<TokenTree> = ts.into_iter().collect();
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                TokenTree::Ident(id) => {
                    let word = id.to_string();
                    if word == "unsafe" {
                        self.hazard("`unsafe` in macro tokens", id.span());
                    }
                    if word == "extern" {
                        self.hazard("`extern` in macro tokens", id.span());
                    }
                    if matches!(word.as_str(), "include" | "include_str" | "include_bytes")
                        && matches!(toks.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                    {
                        self.hazard(
                            format!(
                                "`{word}!` inside macro tokens, whose target cannot be followed"
                            ),
                            id.span(),
                        );
                    }
                    // `. name (` is a method call.
                    let after_dot =
                        i > 0 && matches!(&toks[i - 1], TokenTree::Punct(p) if p.as_char() == '.');
                    if after_dot {
                        let m = MethodCall {
                            name: word.clone(),
                            test: self.test(),
                            site: self.site(id.span()),
                            in_fn: self.in_fn(),
                            in_worker: self.worker_depth > 0,
                        };
                        self.ws.modules[self.module].methods.push(m);
                    }
                    // A run `a :: b :: c` (possibly led by `::`).
                    let mut segs = vec![word];
                    let mut j = i;
                    while j + 3 < toks.len() + 1
                        && matches!(toks.get(j + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':')
                        && matches!(toks.get(j + 2), Some(TokenTree::Punct(p)) if p.as_char() == ':')
                    {
                        match toks.get(j + 3) {
                            Some(TokenTree::Ident(n)) => {
                                segs.push(n.to_string());
                                j += 3;
                            }
                            Some(TokenTree::Punct(p)) if p.as_char() == '<' => break,
                            _ => break,
                        }
                    }
                    if !after_dot && (segs.len() > 1 || is_extern_crate(&segs[0])) {
                        self.push_ref(segs, RefKind::Code, id.span());
                    } else if !after_dot && segs.len() == 1 {
                        // A bare identifier in a macro body can still be
                        // an imported item used as a value.
                        self.push_ref(segs, RefKind::Code, id.span());
                    }
                    i = j;
                }
                TokenTree::Literal(l) => {
                    if let Ok(syn::Lit::Str(s)) = syn::parse_str::<syn::Lit>(&l.to_string()) {
                        let lit = Literal {
                            value: s.value(),
                            test: self.test(),
                            site: self.site(l.span()),
                        };
                        self.ws.modules[self.module].literals.push(lit);
                    }
                }
                TokenTree::Punct(_) => {}
            }
            i += 1;
        }
    }

    /// An `include!`-family invocation: recorded with its resolved
    /// target, and for `include!` the target's items are visited as part
    /// of this module -- the compiler splices them in here.
    fn include(&mut self, kind: &str, m: &syn::Macro) {
        let file = self.ws.modules[self.module].file.clone();
        let target = syn::parse2::<syn::LitStr>(m.tokens.clone())
            .ok()
            .map(|lit| {
                let v = lit.value();
                if v.starts_with('/') {
                    v
                } else {
                    normalize(&format!("{}/{v}", parent_dir(&file)))
                }
            });
        let inc = Include {
            kind: kind.to_string(),
            target: target.clone(),
            test: self.test(),
            site: self.site(m.path.span()),
        };
        self.ws.modules[self.module].includes.push(inc);
        let (Some(target), "include") = (target, kind) else {
            return;
        };
        if !self.ws.included.insert(target.clone()) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(self.ws.root.join(&target)) else {
            return;
        };
        if let Ok(parsed) = parse_cached(&text) {
            for item in &parsed.items {
                if let syn::Item::Mod(md) = item {
                    self.hazard(
                        format!("`mod {}` declared inside `include!`d {target}", md.ident),
                        md.span(),
                    );
                }
                self.visit_item(item);
            }
        } else if let Ok(e) = syn::parse_str::<syn::Expr>(&text) {
            self.visit_expr(&e);
        } else {
            self.ws
                .parse_errors
                .push(format!("{target}: `include!`d file does not parse"));
        }
    }

    fn pub_item(&mut self, vis: &syn::Visibility, ident: &syn::Ident) {
        if matches!(vis, syn::Visibility::Public(_)) && !self.test() {
            let site = self.site(ident.span());
            self.ws.modules[self.module]
                .pub_items
                .push((ident.to_string(), site));
        }
    }

    fn serialized_names(&mut self, attrs: &[syn::Attribute], owner: &str, fields: &syn::Fields) {
        if !derives(attrs, "Serialize") {
            return;
        }
        for f in fields {
            if let Some(id) = &f.ident {
                if has_serde(&f.attrs, "skip") || has_serde(&f.attrs, "skip_serializing") {
                    continue;
                }
                let s = SerializedName {
                    owner: owner.to_string(),
                    name: id.to_string(),
                    test: self.test() || attrs_test(&f.attrs),
                    site: self.site(id.span()),
                };
                self.ws.modules[self.module].serialized.push(s);
            }
        }
    }
}

fn derives(attrs: &[syn::Attribute], what: &str) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("derive")
            && quote::ToTokens::to_token_stream(&a.meta)
                .to_string()
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|t| t == what)
    })
}

fn has_serde(attrs: &[syn::Attribute], word: &str) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("serde")
            && quote::ToTokens::to_token_stream(&a.meta)
                .to_string()
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|t| t == word)
    })
}

fn path_segments(p: &syn::Path) -> Vec<String> {
    p.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// Macros whose arguments are comma-separated expressions (so they can
/// be visited as real syntax: literals, calls, paths).
fn expr_macro(name: &str) -> bool {
    matches!(
        name,
        "format"
            | "print"
            | "println"
            | "eprint"
            | "eprintln"
            | "write"
            | "writeln"
            | "panic"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "vec"
            | "bail"
            | "anyhow"
            | "ensure"
            | "format_args"
            | "concat"
            | "todo"
            | "unimplemented"
            | "unreachable"
            | "dbg"
    )
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.with_attrs(&f.attrs, |c| {
            if let Some(abi) = &f.sig.abi {
                c.hazard(
                    format!(
                        "`extern {}` fn `{}`",
                        abi.name.as_ref().map(|n| n.value()).unwrap_or_default(),
                        f.sig.ident
                    ),
                    f.sig.span(),
                );
            }
            if f.sig.unsafety.is_some() {
                c.hazard(format!("`unsafe fn {}`", f.sig.ident), f.sig.span());
            }
            c.begin_fn(
                f.sig.ident.to_string(),
                matches!(f.vis, syn::Visibility::Public(_)),
                f.sig.ident.span(),
            );
            syn::visit::visit_signature(c, &f.sig);
            c.visit_block(&f.block);
            c.fn_stack.pop();
        });
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.with_attrs(&f.attrs, |c| {
            if f.sig.unsafety.is_some() {
                c.hazard(format!("`unsafe fn {}`", f.sig.ident), f.sig.span());
            }
            if let Some(abi) = &f.sig.abi {
                c.hazard(
                    format!(
                        "`extern {}` fn `{}`",
                        abi.name.as_ref().map(|n| n.value()).unwrap_or_default(),
                        f.sig.ident
                    ),
                    f.sig.span(),
                );
            }
            c.begin_fn(
                f.sig.ident.to_string(),
                matches!(f.vis, syn::Visibility::Public(_)),
                f.sig.ident.span(),
            );
            syn::visit::visit_signature(c, &f.sig);
            c.visit_block(&f.block);
            c.fn_stack.pop();
        });
    }

    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        self.with_attrs(&f.attrs, |c| {
            c.begin_fn(f.sig.ident.to_string(), false, f.sig.ident.span());
            syn::visit::visit_signature(c, &f.sig);
            if let Some(b) = &f.default {
                c.visit_block(b);
            }
            c.fn_stack.pop();
        });
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.with_attrs(&i.attrs, |c| {
            if i.unsafety.is_some() {
                c.hazard("`unsafe impl`", i.span());
            }
            let owner = match &*i.self_ty {
                syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
                _ => None,
            };
            let prev = std::mem::replace(&mut c.impl_owner, owner);
            let prev_trait = c.in_trait_impl;
            let prev_impl_trait = c.impl_trait.take();
            if let Some((_, p, _)) = &i.trait_ {
                c.push_ref(path_segments(p), RefKind::Code, p.span());
                c.in_trait_impl = true;
                c.impl_trait = p.segments.last().map(|s| s.ident.to_string());
            }
            c.visit_type(&i.self_ty);
            c.visit_generics(&i.generics);
            for item in &i.items {
                c.visit_impl_item(item);
            }
            c.impl_owner = prev;
            c.in_trait_impl = prev_trait;
            c.impl_trait = prev_impl_trait;
        });
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.with_attrs(&t.attrs, |c| {
            if t.unsafety.is_some() {
                c.hazard("`unsafe trait`", t.span());
            }
            let prev = c.impl_owner.replace(t.ident.to_string());
            let prev_def = std::mem::replace(&mut c.in_trait_def, true);
            if matches!(t.vis, syn::Visibility::Public(_)) && !c.test() {
                let site = c.site(t.ident.span());
                c.ws.modules[c.module]
                    .pub_items
                    .push((t.ident.to_string(), site));
            }
            syn::visit::visit_item_trait(c, t);
            c.impl_owner = prev;
            c.in_trait_def = prev_def;
        });
    }

    fn visit_item_foreign_mod(&mut self, f: &'ast syn::ItemForeignMod) {
        self.with_attrs(&f.attrs, |c| {
            c.hazard("`extern` block", f.span());
            syn::visit::visit_item_foreign_mod(c, f);
        });
    }

    fn visit_item_extern_crate(&mut self, e: &'ast syn::ItemExternCrate) {
        self.with_attrs(&e.attrs, |c| {
            c.push_ref(vec![e.ident.to_string()], RefKind::Use, e.span());
            if let Some((_, alias)) = &e.rename {
                c.ws.modules[c.module].uses.push(UseBinding {
                    alias: alias.to_string(),
                    target: vec![e.ident.to_string()],
                });
            }
        });
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.with_attrs(&u.attrs, |c| {
            // `use ::x` and `use x` start from the same (empty) prefix.
            c.use_tree(&u.tree, Vec::new(), u.span());
        });
    }

    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        self.with_attrs(&s.attrs, |c| {
            c.pub_item(&s.vis, &s.ident);
            c.serialized_names(&s.attrs, &s.ident.to_string(), &s.fields);
            syn::visit::visit_item_struct(c, s);
        });
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        self.with_attrs(&e.attrs, |c| {
            c.pub_item(&e.vis, &e.ident);
            if derives(&e.attrs, "Serialize") {
                for v in &e.variants {
                    if has_serde(&v.attrs, "skip") {
                        continue;
                    }
                    let s = SerializedName {
                        owner: e.ident.to_string(),
                        name: v.ident.to_string(),
                        test: c.test(),
                        site: c.site(v.ident.span()),
                    };
                    c.ws.modules[c.module].serialized.push(s);
                    let owner = format!("{}::{}", e.ident, v.ident);
                    c.serialized_names(&e.attrs, &owner, &v.fields);
                }
            }
            syn::visit::visit_item_enum(c, e);
        });
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.with_attrs(&i.attrs, |c| {
            if c.fn_stack.is_empty() {
                c.pub_item(&i.vis, &i.ident);
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(v),
                    ..
                }) = &*i.expr
                {
                    let site = c.site(i.ident.span());
                    c.ws.modules[c.module]
                        .str_consts
                        .push((i.ident.to_string(), v.value(), site));
                }
            }
            syn::visit::visit_item_const(c, i)
        });
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.with_attrs(&i.attrs, |c| {
            if c.fn_stack.is_empty() {
                c.pub_item(&i.vis, &i.ident);
            }
            syn::visit::visit_item_static(c, i)
        });
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.with_attrs(&i.attrs, |c| {
            c.pub_item(&i.vis, &i.ident);
            syn::visit::visit_item_type(c, i)
        });
    }

    fn visit_item_macro(&mut self, m: &'ast syn::ItemMacro) {
        self.with_attrs(&m.attrs, |c| {
            // `macro_rules! name { .. }`: the body is never "not a call
            // site" -- whatever it names, it names.
            if m.mac.path.is_ident("macro_rules") {
                c.scan_tokens(m.mac.tokens.clone());
            } else {
                c.visit_macro(&m.mac);
            }
        });
    }

    fn visit_stmt(&mut self, s: &'ast syn::Stmt) {
        match s {
            syn::Stmt::Local(l) => self.with_attrs(&l.attrs, |c| c.visit_local(l)),
            syn::Stmt::Macro(m) => self.with_attrs(&m.attrs, |c| c.visit_macro(&m.mac)),
            syn::Stmt::Item(i) => self.visit_item(i),
            syn::Stmt::Expr(e, _) => self.visit_expr(e),
        }
    }

    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        // Attributes on expressions (`#[cfg(test)] { .. }`).
        let attrs: &[syn::Attribute] = match e {
            syn::Expr::Block(b) => &b.attrs,
            syn::Expr::Call(c) => &c.attrs,
            syn::Expr::MethodCall(m) => &m.attrs,
            syn::Expr::Macro(m) => &m.attrs,
            _ => &[],
        };
        if !attrs.is_empty() {
            self.with_attrs(attrs, |c| syn::visit::visit_expr(c, e));
        } else {
            syn::visit::visit_expr(self, e);
        }
    }

    fn visit_expr_unsafe(&mut self, u: &'ast syn::ExprUnsafe) {
        self.hazard("`unsafe` block", u.span());
        syn::visit::visit_expr_unsafe(self, u);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        // A closure called on the spot is analyzed inline; a path is a
        // path. Anything else -- a field, an index, a call's result -- is a
        // value whose target this model cannot name.
        let mut callee = &*call.func;
        while let syn::Expr::Paren(p) = callee {
            callee = &p.expr;
        }
        if let syn::Expr::Path(p) = callee
            && !self.test()
        {
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            let n = segs.len();
            let path_ctor = n >= 2
                && matches!(segs[n - 2].as_str(), "PathBuf" | "Path")
                && matches!(segs[n - 1].as_str(), "from" | "new");
            if path_ctor
                && let Some(syn::Expr::Lit(l)) = call.args.first()
                && let syn::Lit::Str(v) = &l.lit
                && (v.value().starts_with('/') || v.value().starts_with("~/"))
            {
                let site = self.site(l.span());
                self.ws.modules[self.module]
                    .path_literals
                    .push((v.value(), site));
            }
        }
        let opaque = !matches!(callee, syn::Expr::Path(_) | syn::Expr::Closure(_));
        if opaque
            && self.worker_depth == 0
            && let Some(f) = self.in_fn()
        {
            let site = self.site(call.func.span());
            self.ws.fns[f].opaque_calls.push(site);
        }
        // `worker::spawn(job)`: the TUI's one way off the event thread.
        let spawn = matches!(callee, syn::Expr::Path(p)
            if (p.path.segments.last().is_some_and(|s| s.ident == "spawn")
                && p.path.segments.iter().any(|s| s.ident == "worker"))
                || (p.path.segments.len() == 1
                    && self.spawn_aliases.contains(&p.path.segments[0].ident.to_string())));
        if spawn {
            self.visit_expr(&call.func);
            self.worker_depth += 1;
            for a in &call.args {
                self.visit_expr(a);
            }
            self.worker_depth -= 1;
            return;
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_struct(&mut self, e: &'ast syn::ExprStruct) {
        let mut segments = path_segments(&e.path);
        if segments.first().is_some_and(|s| s == "Self")
            && let Some(owner) = &self.impl_owner
        {
            segments[0] = owner.clone();
        }
        let lit = StructLiteral {
            segments,
            test: self.test(),
            site: self.site(e.path.span()),
            in_fn: self.in_fn(),
        };
        self.ws.modules[self.module].struct_literals.push(lit);
        syn::visit::visit_expr_struct(self, e);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        let mc = MethodCall {
            name: m.method.to_string(),
            test: self.test(),
            site: self.site(m.method.span()),
            in_fn: self.in_fn(),
            in_worker: self.worker_depth > 0,
        };
        self.ws.modules[self.module].methods.push(mc);
        syn::visit::visit_expr_method_call(self, m);
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let mut segs = path_segments(p);
        if p.leading_colon.is_some()
            && let Some(first) = segs.first()
            && !is_extern_crate(first)
        {
            segs.insert(0, String::new());
        }
        self.push_ref(segs, RefKind::Code, p.span());
        syn::visit::visit_path(self, p);
    }

    fn visit_qself(&mut self, q: &'ast syn::QSelf) {
        // `<std::process::Command>::new`: the self type is a reference.
        self.visit_type(&q.ty);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if let syn::Pat::Ident(id) = &l.pat
            && let Some(init) = &l.init
            && let syn::Expr::Path(p) = &*init.expr
            && p.path.segments.last().is_some_and(|s| s.ident == "spawn")
            && p.path.segments.iter().any(|s| s.ident == "worker")
        {
            self.spawn_aliases.insert(id.ident.to_string());
        }
        syn::visit::visit_local(self, l);
    }

    fn visit_lit_str(&mut self, l: &'ast syn::LitStr) {
        let lit = Literal {
            value: l.value(),
            test: self.test(),
            site: self.site(l.span()),
        };
        self.ws.modules[self.module].literals.push(lit);
    }

    fn visit_attribute(&mut self, a: &'ast syn::Attribute) {
        // Doc comments are prose, not code or delivered strings; every
        // other attribute's paths are not calls either. `#[path]`,
        // `#[no_mangle]`, `#[link]`, `#[export_name]` are hazards.
        let p = a.path();
        if p.is_ident("no_mangle") || p.is_ident("export_name") || p.is_ident("link") {
            self.hazard(
                format!("`#[{}]`", quote::ToTokens::to_token_stream(p)),
                a.span(),
            );
        }
        if p.is_ident("path") {
            self.hazard("`#[path]`", a.span());
        }
        if (p.is_ident("allow")
            || p.is_ident("expect")
            || p.is_ident("warn")
            || p.is_ident("cfg_attr"))
            && let Some(what) = lowers_gate_lint(quote::ToTokens::to_token_stream(&a.meta))
        {
            let site = self.site(a.span());
            let test = self.test();
            self.ws.modules[self.module]
                .lint_allows
                .push((what, site, test));
        }
        if (p.is_ident("allow") || p.is_ident("expect"))
            && !self.test()
            && let syn::Meta::List(l) = &a.meta
            && l.tokens
                .clone()
                .into_iter()
                .any(|t| t.to_string() == "dead_code")
        {
            let site = self.site(a.span());
            self.ws.modules[self.module].dead_code_allows.push(site);
        }
        // Every other attribute's tokens are names and strings too:
        // `#[serde(rename = "safe_to_delete")]` delivers that string, and
        // `#[serde(default = "f")]`/`#[arg(value_parser = f)]` name `f`.
        if !p.is_ident("doc")
            && let syn::Meta::List(l) = &a.meta
        {
            self.scan_tokens(l.tokens.clone());
        }
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let name = m
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let mc = MacroCall {
            name: name.clone(),
            test: self.test(),
            site: self.site(m.path.span()),
            in_fn: self.in_fn(),
        };
        self.ws.modules[self.module].macros.push(mc);
        if matches!(name.as_str(), "asm" | "global_asm" | "naked_asm") {
            self.hazard(format!("`{name}!`"), m.span());
        }
        if matches!(name.as_str(), "include" | "include_str" | "include_bytes") {
            self.include(&name, m);
            return;
        }
        self.push_ref(path_segments(&m.path), RefKind::Code, m.path.span());
        if expr_macro(&name)
            && let Ok(args) = m.parse_body_with(
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
            )
        {
            for e in &args {
                self.visit_expr(e);
            }
            // `concat!("can", " be deleted")` delivers one string: record
            // the joined text too, so a phrase split across arguments is
            // still the phrase.
            if name == "concat" {
                let mut joined = String::new();
                for e in &args {
                    if let syn::Expr::Lit(l) = e
                        && let syn::Lit::Str(s) = &l.lit
                    {
                        joined.push_str(&s.value());
                    }
                }
                let lit = Literal {
                    value: joined,
                    test: self.test(),
                    site: self.site(m.path.span()),
                };
                self.ws.modules[self.module].literals.push(lit);
            }
            return;
        }
        if name == "matches"
            && let Ok(args) = m.parse_body_with(
                syn::punctuated::Punctuated::<MatchesArg, syn::Token![,]>::parse_terminated,
            )
        {
            for a in &args {
                match a {
                    MatchesArg::Expr(e) => self.visit_expr(e),
                    MatchesArg::Pat(p) => self.visit_pat(p),
                }
            }
            return;
        }
        self.scan_tokens(m.tokens.clone());
    }
}

/// `matches!(expr, pattern)` arguments.
enum MatchesArg {
    Expr(syn::Expr),
    Pat(syn::Pat),
}

impl syn::parse::Parse for MatchesArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let fork = input.fork();
        if fork.parse::<syn::Expr>().is_ok() && (fork.is_empty() || fork.peek(syn::Token![,])) {
            return Ok(MatchesArg::Expr(input.parse()?));
        }
        Ok(MatchesArg::Pat(syn::Pat::parse_multi_with_leading_vert(
            input,
        )?))
    }
}

// ---------------------------------------------------------------------
// files
// ---------------------------------------------------------------------

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `.rs` file under `dir`, recursively, sorted.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// A parsed test file (`crates/*/tests/**.rs`), for the guardrail and
/// contract rules: every `#[test]` fn with whether it is ignored and
/// the macros its body invokes.
#[derive(Debug, Clone)]
pub struct TestFn {
    pub file: String,
    pub name: String,
    pub ignored: bool,
    /// Macro names invoked in the body (`assert`, `assert_eq`, ...).
    pub macros: Vec<String>,
    /// Paths called in the body (`contract::unknown_layout_is_explicit`).
    pub calls: Vec<String>,
}

/// Every `#[test]` fn Cargo builds: those in `crates/*/tests/*.rs`
/// (top-level files are the test targets) and in modules they declare,
/// plus the `#[cfg(test)]` code reached from each crate root.
pub fn compiled_tests(root: &Path) -> (Vec<TestFn>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let Ok(crates) = std::fs::read_dir(root.join("crates")) else {
        return (out, errors);
    };
    let mut crate_dirs: Vec<PathBuf> = crates.flatten().map(|e| e.path()).collect();
    crate_dirs.sort();
    for c in crate_dirs {
        // Integration test targets: top-level files only.
        let tests = c.join("tests");
        if let Ok(rd) = std::fs::read_dir(&tests) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "rs"))
                .collect();
            files.sort();
            for f in files {
                let mut reached = HashSet::new();
                collect_test_file(root, &f, &mut out, &mut errors, &mut reached);
            }
        }
        // Unit tests: the crate's own module tree.
        for top in ["src/lib.rs", "src/main.rs"] {
            let f = c.join(top);
            if f.is_file() {
                let mut reached = HashSet::new();
                collect_test_file(root, &f, &mut out, &mut errors, &mut reached);
            }
        }
    }
    (out, errors)
}

fn collect_test_file(
    root: &Path,
    file: &Path,
    out: &mut Vec<TestFn>,
    errors: &mut Vec<String>,
    reached: &mut HashSet<PathBuf>,
) {
    if !reached.insert(file.to_path_buf()) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let parsed = match parse_cached(&text) {
        Ok(p) => p,
        Err(e) => {
            errors.push(format!("{}: {e}", rel(root, file)));
            return;
        }
    };
    let relf = rel(root, file);
    let dir = child_dir(&relf);
    collect_test_items(root, &relf, &dir, &parsed.items, out, errors, reached);
}

fn collect_test_items(
    root: &Path,
    relf: &str,
    dir: &str,
    items: &[syn::Item],
    out: &mut Vec<TestFn>,
    errors: &mut Vec<String>,
    reached: &mut HashSet<PathBuf>,
) {
    for item in items {
        match item {
            syn::Item::Fn(f) if f.attrs.iter().any(|a| a.path().is_ident("test")) => {
                let mut v = BodyVisitor::default();
                v.visit_block(&f.block);
                out.push(TestFn {
                    file: relf.to_string(),
                    name: f.sig.ident.to_string(),
                    ignored: attrs_ignored(&f.attrs),
                    macros: v.macros,
                    calls: v.calls,
                });
            }
            syn::Item::Mod(m) => {
                let name = m.ident.to_string();
                match &m.content {
                    Some((_, inner)) => {
                        let d = format!("{dir}/{name}");
                        collect_test_items(root, relf, &d, inner, out, errors, reached);
                    }
                    None => {
                        let explicit = path_attr(&m.attrs);
                        let cands: Vec<String> = match explicit {
                            Some(p) => vec![format!("{}/{p}", parent_dir(relf))],
                            None => {
                                vec![format!("{dir}/{name}.rs"), format!("{dir}/{name}/mod.rs")]
                            }
                        };
                        if let Some(f) = cands.iter().map(|c| root.join(c)).find(|p| p.is_file()) {
                            collect_test_file(root, &f, out, errors, reached);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct BodyVisitor {
    macros: Vec<String>,
    calls: Vec<String>,
}

impl<'ast> Visit<'ast> for BodyVisitor {
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if let Some(s) = m.path.segments.last() {
            self.macros.push(s.ident.to_string());
        }
        if let Ok(args) = m.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        ) {
            for e in &args {
                self.visit_expr(e);
            }
        }
    }
    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func {
            self.calls.push(
                p.path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
            );
        }
        syn::visit::visit_expr_call(self, c);
    }
}

/// `BTreeMap` of absolute-path-prefix → count, for diagnostics.
pub type Counts = BTreeMap<String, usize>;

/// Every resolved absolute path each production module references, as
/// strings (`std::fs::read_dir`, `@core::fs_gate::destroy::trash_move`).
pub fn resolved_refs(ws: &Workspace) -> Vec<(usize, &Ref, String)> {
    let mut out = Vec::new();
    for (mi, m) in ws.modules.iter().enumerate() {
        for r in &m.refs {
            let abs = ws.resolve(mi, &r.segments).join("::");
            out.push((mi, r, abs));
        }
    }
    out
}

/// Whether `abs` is `prefix` or lies beneath it (segment-wise).
pub fn under(abs: &str, prefix: &str) -> bool {
    abs == prefix || abs.starts_with(&format!("{prefix}::"))
}

/// Methods defined in workspace impls and traits, by name.
pub fn methods_by_name(ws: &Workspace) -> HashMap<String, Vec<usize>> {
    let mut out: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in ws.fns.iter().enumerate() {
        if f.owner.is_some() {
            out.entry(f.name.clone()).or_default().push(i);
        }
    }
    out
}

thread_local! {
    static PARSED: std::cell::RefCell<HashMap<String, Result<std::rc::Rc<syn::File>, String>>> =
        std::cell::RefCell::new(HashMap::new());
    static IDENTS: std::cell::RefCell<HashMap<String, std::rc::Rc<HashSet<String>>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// `syn::parse_file`, memoized by the file's text. The mutation harness
/// loads the workspace once per mutation, and all but one or two files
/// are unchanged between loads; re-parsing them is most of the cost, and
/// every parse also grows proc-macro2's span source map for good.
pub fn parse_cached(text: &str) -> Result<std::rc::Rc<syn::File>, String> {
    if let Some(hit) = PARSED.with(|c| c.borrow().get(text).cloned()) {
        return hit;
    }
    let parsed = syn::parse_file(text)
        .map(std::rc::Rc::new)
        .map_err(|e| e.to_string());
    PARSED.with(|c| c.borrow_mut().insert(text.to_string(), parsed.clone()));
    parsed
}

fn idents_cached(text: &str) -> std::rc::Rc<HashSet<String>> {
    if let Some(hit) = IDENTS.with(|c| c.borrow().get(text).cloned()) {
        return hit;
    }
    let mut set = HashSet::new();
    if let Ok(ts) = text.parse::<TokenStream>() {
        collect_idents(ts, &mut set);
    }
    let set = std::rc::Rc::new(set);
    IDENTS.with(|c| c.borrow_mut().insert(text.to_string(), set.clone()));
    set
}

/// Every identifier in the workspace's integration test files
/// (`crates/*/tests/**.rs`), token-level: what a test names keeps a
/// public item referenced.
pub fn test_identifiers(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(crates) = std::fs::read_dir(root.join("crates")) else {
        return out;
    };
    for c in crates.flatten() {
        for f in rust_files(&c.path().join("tests")) {
            // Mutation fixtures and compile-fail cases are data, not code
            // Cargo builds: what they name is not thereby used.
            if f.components()
                .any(|c| matches!(c.as_os_str().to_str(), Some("mutations" | "compile_fail")))
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            out.extend(idents_cached(&text).iter().cloned());
        }
    }
    out
}

fn collect_idents(ts: TokenStream, out: &mut HashSet<String>) {
    for t in ts {
        match t {
            TokenTree::Ident(i) => {
                out.insert(i.to_string());
            }
            TokenTree::Group(g) => collect_idents(g.stream(), out),
            _ => {}
        }
    }
}
