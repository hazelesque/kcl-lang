# In-memory module resolution: loader injection points (Phase 3 step 1)

**Status:** investigation memo. Records the decision for how the embedding API's `register_module(path, source)` plumbs through the parser's import-resolution machinery.

**Decision:** extend `kcl_parser::LoadProgramOptions` with a `virtual_packages: HashMap<String, VirtualPackage>` field. Add a new `is_virtual_pkg` step in `find_packages` that runs before the disk-based lookups. Leverage the existing `KCLModuleCache::source_code` map to override file-content reads for synthetic paths.

---

## What I needed to figure out

KCL's parser resolves `import foo.bar` statements during program loading. To embed schemas in memory (no tempdir, no on-disk artefacts), the embedding API has to inject host-registered modules into that resolution flow at the right seam. The plan's D3 budgeted a half-day for this investigation; this memo is the output.

The wrong answer (D3 originally hypothesised) was that `LoadProgramOptions::k_code_list` would be enough. It isn't: `k_code_list` is consumed pairwise with top-level `file_paths` (`crates/parser/src/entry.rs:302`); it does not participate in transitive import resolution.

## How import resolution actually works

Walking the import path from top-level `load_program` to file content:

1. **`load_program`** (`crates/parser/src/lib.rs:311`) constructs a `Loader` and calls `load_main` → `_load_main` → `parse_program`.

2. **`parse_program`** (`lib.rs:931`) gathers top-level compile entries via `get_compile_entries_from_paths` (which consumes `k_code_list`), then calls `parse_entry` for each. As parsing discovers `import` statements, it queues dependencies into the `file_graph`. The file_graph is toposorted to produce the parse order.

3. **`fix_rel_import_path_with_file`** (`lib.rs:396`) is called per module to rewrite each `import_spec`'s path. For each import, it calls `find_packages` to locate the imported package.

4. **`find_packages`** (`lib.rs:444`) is the resolution choke point. In order:
   - Plugin packages (`plugin.foo`) — handled specially via `is_plugin_pkg`.
   - Built-in packages (`yaml`, `json`, `regex`, etc.) — handled by `is_builtin_pkg`; returns `Ok(None)` (the runtime provides these intrinsically).
   - **Internal packages**: `is_internal_pkg(pkg_name, pkg_root, pkg_path)` — looks in the current pkg's root directory.
   - **External packages**: `is_external_pkg(pkg_path, opts)` — uses `opts.package_maps.get(pkg_name)` to look up an on-disk pkg_root, falls back to scanning `opts.vendor_dirs`.
   - "Found in both internal and external" is an error; "found in neither" is the "pkgpath not found" diagnostic users see today.

5. **`is_external_pkg`** (`lib.rs:657`) is the closest pre-existing extension point. It:
   - Looks up `pkg_name` in `opts.package_maps`.
   - Joins the value with `KCL_MOD_FILE` (`kcl.mod`).
   - Checks `external_pkg_root.exists()` — **filesystem check; this is what blocks fully-virtual packages**.
   - Walks `get_dir_files` (reads `*.k` files via `std::fs::read_dir` + filtering) to enumerate the package's .k files.

6. **`parse_file`** (`lib.rs:694`) is the file-content seam:
   ```rust
   let src = match src {
       Some(src) => Some(src),
       None => match &module_cache.read() {
           Ok(cache) => cache.source_code.get(file.get_path()),
           Err(_) => None,
       }.cloned(),
   };
   let m = parse_file_with_session(sess.clone(), file.get_path().to_str().unwrap(), src)?;
   ```
   If `src` is provided explicitly *or* if `module_cache.source_code` contains an entry for the file's path, the on-disk read in `parse_file_with_session` is bypassed. **This pre-existing mechanism is what LSP uses for unsaved buffers** and is exactly what we need for in-memory module bodies.

## Options considered

### Option A — Tempfiles

Write registered sources to a tempdir at startup; pre-populate `opts.package_maps` with the tempdir paths; let `is_external_pkg` discover them via the existing filesystem check.

This is exactly what **Tilley does today** at `tilley/src/config.rs:334–348`, and the explicit goal of Phase 3 is to eliminate it. Rejected.

### Option B — Extend `LoadProgramOptions` with virtual packages

Add a new field:

```rust
pub struct LoadProgramOptions {
    // ... existing fields ...
    pub virtual_packages: HashMap<String, VirtualPackage>,
}

pub struct VirtualPackage {
    /// Synthetic root used for module-path resolution.
    /// Conventionally `/__kcl_embed__/<pkg_name>` — does not need to exist
    /// on disk; treated as opaque identifier.
    pub root: PathBuf,
    /// (synthetic_file_path, source_text) pairs. The synthetic_file_path
    /// is what gets registered into `module_cache.source_code` so the
    /// parse_file lookup at lib.rs:706 finds the in-memory content.
    pub files: Vec<(PathBuf, String)>,
}
```

Add a new step in `find_packages`:

```rust
// 0. Check virtual packages first; overrides disk per D3's shadowing rule.
if let Some(pkg_info) = is_virtual_pkg(pkg_path, opts) {
    return Ok(Some(pkg_info));
}
```

`is_virtual_pkg` constructs a `PkgInfo` from the in-memory file list directly — no `external_pkg_root.exists()` check, no `get_dir_files` call.

Pre-populate `module_cache.source_code` with the `(synthetic_file_path, source)` pairs at embedding-API setup time. `parse_file` (lib.rs:694) already does the right thing: tries `src` argument, then `module_cache.source_code`, then disk. The synthetic paths never exist on disk, but `module_cache.source_code` hits first.

**Pros:**
- Reuses an existing mechanism (`module_cache.source_code`) — minimal new code.
- Single new field on `LoadProgramOptions` — minimally invasive API change.
- No tempfiles, no `unsafe`, no trait-object dispatch.
- Shadowing semantics (per D3) fall out of step ordering: virtual packages checked first → in-memory wins over disk for matching paths.

**Cons:**
- One small thing to watch: `is_external_pkg`'s "found in both internal and external" check needs an analogous "found in both virtual and disk" branch. Otherwise registering a module path that also exists on disk silently picks the virtual one without diagnosing the collision. Per D3 the in-memory registry shadows disk, so silent is OK — but the behaviour is documented in the memo and in the embed crate's rustdoc.

### Option C — Trait-object `PackageResolver` plug

Replace `find_packages` with a delegation through `Box<dyn PackageResolver>` configured on `LoadProgramOptions`. The default impl does the disk lookups; the embedding API supplies a different impl.

**Pros:** most flexible; arbitrary resolution strategies become possible (HTTP, in-memory cache, IPC, etc.).

**Cons:** adds dispatch overhead in the resolution hot path. Adds a public trait surface to the parser crate. Tilley does not need this flexibility; future Hazel can promote Option B to Option C if/when a third consumer appears with different resolution requirements.

Rejected as over-engineering for current needs.

## Decision: Option B

Concrete signatures:

```rust
// crates/parser/src/lib.rs

#[derive(Debug, Clone, Default)]
pub struct VirtualPackage {
    pub root: PathBuf,
    pub files: Vec<(PathBuf, String)>,
}

pub struct LoadProgramOptions {
    // ... existing fields ...
    /// Packages whose source is provided in-memory by the embedding
    /// host (see crates/embed/ Embedded::register_module). Resolves
    /// strictly before disk-based external and internal package
    /// lookups; shadowing is intentional.
    pub virtual_packages: HashMap<String, VirtualPackage>,
}

fn is_virtual_pkg(pkg_path: &str, opts: &LoadProgramOptions) -> Option<PkgInfo> {
    let pkg_name = parse_external_pkg_name(pkg_path).ok()?;
    let virt = opts.virtual_packages.get(&pkg_name)?;
    let k_files = virt
        .files
        .iter()
        .map(|(p, _)| p.to_string_lossy().to_string())
        .collect();
    Some(PkgInfo::new(
        pkg_name,
        virt.root.to_string_lossy().to_string(),
        pkg_path.to_string(),
        k_files,
    ))
}
```

Modification site in `find_packages` (`lib.rs:444`):

```rust
fn find_packages(...) -> Result<Option<PkgInfo>> {
    if pkg_path.is_empty() { return Ok(None); }
    if is_plugin_pkg(pkg_path) { /* unchanged */ }
    if is_builtin_pkg(pkg_path) { return Ok(None); }

    // NEW: virtual packages take precedence over disk.
    if let Some(pkg_info) = is_virtual_pkg(pkg_path, opts) {
        return Ok(Some(pkg_info));
    }

    // ... existing is_internal_pkg + is_external_pkg flow unchanged.
}
```

The embedding API (Phase 3 step 3+) populates two things at `EmbeddedReady::evaluate` time:
- `opts.virtual_packages` map, one entry per `register_module` call.
- `module_cache.source_code` entries for each synthetic file path.

Both are passed through `LoadProgramOptions` / `KCLModuleCache` to `load_program`. No new threading required.

## Tests required for step 2

Per the plan's Phase 3 step 2 verification:

1. **Main source imports a registered module.** `register_module("tilley", source)`; main program does `import tilley`; resolution succeeds; uses fields/schemas declared in the registered source.

2. **Transitive import across registered modules.** `register_module("tilley", "import tilley.vm; ...")`; `register_module("tilley.vm", "...")`; main imports `tilley`; the loader follows the transitive `import tilley.vm`.

3. **In-memory shadows disk for matching paths.** Place a `tilley` package on disk via the test data dir; also `register_module("tilley", different_source)`; assert the in-memory content is what gets resolved.

4. **Negative: registered-but-never-imported.** `register_module` a module that the main program does not import; assert it does not appear in the program; assert no errors (lazy parsing posture per D3).

5. **Negative: imported-but-not-registered.** `import tilley` without any matching `register_module`; assert the existing "pkgpath not found" diagnostic fires unchanged (no silent fall-through).

## Open questions resolved during investigation

- **Does `module_cache.source_code` actually override disk reads?** Yes (`parse_file` at lib.rs:704–712 explicitly consults it before falling back to `parse_file_with_session`'s disk read).
- **Does `find_packages` need both an `is_virtual_pkg` step AND a `module_cache.source_code` population?** Yes — they're independent layers. `is_virtual_pkg` resolves "pkg_name → list of file paths"; `module_cache.source_code` resolves "file path → source content". Both need the synthetic paths consistent.
- **Will `is_external_pkg`'s "found in both" error fire spuriously?** No: the "found in both" check is *between* `is_internal` and `is_external`; the new `is_virtual_pkg` is a separate earlier step that returns immediately. Disk-side checks don't run if virtual hits.
- **Does the `canonicalize()` call in `is_external_pkg` matter for synthetic paths?** Not on the virtual path — `is_virtual_pkg` doesn't call canonicalize. Synthetic paths flow through `module_cache.source_code` (which is a `HashMap<PathBuf, String>` — keys are compared verbatim, not canonicalized) and through `parse_file_with_session`'s `filename` argument (a display-only `&str`).
- **Does the file_graph care about synthetic vs disk paths?** No — file_graph stores `PathBuf` keys for dependency edges; synthetic paths are valid PathBufs and toposort works the same.
- **Do `pkgmap` / `PkgFile` care?** No — they thread the same `PathBuf`s the parser already uses.

## Implementation order for step 2

1. Add `VirtualPackage` struct + `virtual_packages` field on `LoadProgramOptions`. Default to empty `HashMap`.
2. Implement `is_virtual_pkg` adjacent to `is_external_pkg` in `crates/parser/src/lib.rs`.
3. Wire the `is_virtual_pkg` call into `find_packages` between the builtin/plugin checks and the internal/external resolution.
4. Add the in-memory module source population path in the embedding API (step 3 work — pre-fills `module_cache.source_code` from the registered sources before invoking `load_program`).
5. Tests per the list above, located in `crates/parser/src/tests/` (existing test infrastructure).
6. Integration tests in `crates/embed/` (step 5 work) exercise the end-to-end flow.
