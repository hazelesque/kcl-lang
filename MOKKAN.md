# Mokkan

Mokkan is a private fork of [KCL](https://kcl-lang.io), evolving
toward a typed structured-evaluation language for Hazel's homelab
infrastructure work and (longer-term) a replacement for Google's
Piccolo in that environment.

The name is the Japanese word for *woodwind*; Piccolo is what
we're really after.

## What mokkan extends

- **Native network types**: PostgreSQL-shaped `cidr`, `inet`,
  `macaddr`, `macaddr8`, plus a family-tagged `IpFamily` enum.
  Replaces upstream KCL's stringly-typed `net` package entirely.
  See https://www.postgresql.org/docs/current/datatype-net-types.html
  for the reference data model.

- **A lazy-evaluation mode**: `cidr` and `inet` values can carry
  a symbolic-handle state propagated through a bounded algebra
  (`broadcast`, `host`, `network`, `set_masklen`, `inet_merge`,
  `+ int`, `- int`, …). Stringification of a symbolic value
  produces a `ResolvableString` value carrying an IR tree;
  downstream consumers walk the tree to substitute concrete
  values once they're known. Tilley is the first consumer; future
  Withy work and the monorepo's KCL pipeline are intended
  consumers.

- **Rust-codegen integration** for `Resolvable<T>`: schema fields
  typed as `T | ResolvableString` emit `Resolvable<T>` in the
  generated Rust, with a `walk_resolvables` method per schema
  struct that lets a downstream resolver pass substitute the
  symbolic values.

## Builtin packages

Two packages register under the mokkan namespace, accessed via
the existing KCL dotted-import syntax. KCL's leaf-binding rule
applies — `import mokkan.net` brings `net` into scope at the
call site.

- **`mokkan.net`** — typed inet algebra plus `IpFamily`
  constants:

  ```kcl
  import mokkan.net

  subnet = cidr "10.0.0.0/24"
  host = net.broadcast(subnet)
  is_v4 = net.family(host) == net.V4
  ```

- **`mokkan.net_symbolic`** — symbolic primitives that produce
  deferred `cidr`/`inet` values for runtime resolution:

  ```kcl
  import mokkan.net
  import mokkan.net_symbolic

  lan = net_symbolic.symbolic_subnet("lan", 24, net.V4)
  gateway = net_symbolic.symbolic_inet("lan", 1)
  ```

The upstream KCL `net` package (`net.CIDR_host`,
`net.CIDR_netmask`, etc.) is **removed** — the typed surface
above supersedes it. The non-CIDR upstream functions
(`split_host_port`, `join_host_port`, `fqdn`, …) survive under
`mokkan.net` with their existing signatures where new typed
functionality doesn't already cover them.

## File extension

Mokkan source files use `.k`, same as upstream KCL. High
syntactic compatibility with upstream KCL means renaming every
consumer file would cost more than it'd buy. Tooling (editors,
build systems) keys off `.k` and dispatches to the right
evaluator based on project configuration.

## Why a fork, not upstream contribution

Upstream KCL's design centre is "emit YAML for Helm/Kustomize."
Mokkan's design centre is "typed structured output for Rust
binaries that ingest configuration." Different value-model
priorities (typed Rust consumers benefit from PG-shaped inet
types; YAML-emitting consumers don't), different evaluation
semantics (mokkan's string concatenation propagates
`ResolvableString` segments; upstream KCL has no concept of
deferred values), and different runtime requirements (mokkan's
embedded VM evaluates in-process for Rust hosts; upstream KCL's
focus is the CLI emitting files).

The fork shares syntax and most semantics with upstream KCL but
diverges in type-system extensions and string-concatenation
behaviour. No upstream contribution planned.

## Relationship to upstream KCL

The fork started from upstream KCL's evaluator path and has
diverged through several rounds of additive work:

- Embeddable evaluator (`kcl_embed` crate) for Rust hosts that
  want in-process evaluation.
- Codegen crate (`kcl_rust_codegen`) emitting Rust types from KCL
  schemas, with `# @rust:` annotations for codegen control.
- Tagged-enum codegen for `discriminator`-shaped unions.
- Schema-coercion evaluator fixes uncovered by Tilley's consumer
  work.

The mokkan-named extensions in this README are the current arc
of work. A mechanical sweep renaming the existing `kcl-*` crates
to `mokkan-*` is planned for after the F2 stage of the inet/IPAM
work lands; until then, internal crate names still read as `kcl-*`
to keep cherry-pick mechanics from upstream tractable.

## What mokkan is *not*

- **Not a general-purpose programming language**. KCL's
  declarative shape (single-pass evaluation, no side effects in
  the language proper) is preserved; mokkan extends the value
  model and evaluation semantics, not the imperative surface.
- **Not a CLI tool replacement** for upstream KCL. Mokkan's CLI
  binary is still the upstream one (renamed at the same sweep
  as the crates). Adoption is via the embedded VM, not the CLI.
- **Not a configuration-management runtime**. Mokkan evaluates
  configuration; the consumers (Tilley, future Withy) do the
  actual work of acting on the evaluation result.
