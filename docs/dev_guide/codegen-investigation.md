# Phase 4A codegen pre-investigation memo

**Status:** investigation memo. Records the empirical answers to D5
(optional-field surface) and D10 (mixin codegen strategy) so Phase 4
codegen lands against verified runtime semantics, not hypotheses.

---

## D5: unset optional field → `Value::undefined`

**Empirically resolved before this memo was written.** Documented here
for completeness and so future-Phase-4 reviewers don't have to chase
the resolution across commits.

The plan's original D5 hypothesis was that an unset `field?: T`
declaration would surface as `Value::none` in the resulting
`ValueRef`. The
[Tilley-shaped fixture test](../../crates/runner/src/tests.rs)
(`test_structured_value_tilley_shaped_fixture`, commit `abecf243`)
asserted the hypothesis and **failed loudly with the empirical
answer**: unset optional fields surface as `Value::undefined`, not
`Value::none`.

Codegen mapping:

| Source ValueRef shape       | Generated `Option<T>` mapping          |
|------------------------------|----------------------------------------|
| `Value::undefined`           | `None`  (primary case)                 |
| `Value::none`                | `None`  (defensive — explicit `field = None` literal) |
| `Value::T(v)`                | `Some(v)`                              |
| anything else                | `TryFrom::Err`                         |

The defensive `Value::none → None` arm handles a user explicitly
writing `field = None` in source. Empirically the two shapes are
semantically distinct in the value tree but indistinguishable through
`Option<T>`; that's the contract.

---

## D10: mixin codegen strategy

**Empirically verified by `test_structured_value_mixin_probe` in the
same test file as D5.** The plan's three candidate strategies (trait
+ impl per mixin / inline-into-consuming-schema / codegen-time-error)
narrow to one once the runtime behaviour is known.

### What the runtime actually surfaces

KCL distinguishes `mixin` from `schema` at declaration time:

```kcl
mixin AuditLogMixin:
    audit_id: str
    created_by: str = "system"

schema Foo:
    mixin [AuditLogMixin]
    name: str

foo = Foo {audit_id = "abc123", name = "alpha"}
```

Two **syntactic** constraints surface during the probe:

1. **`mixin` keyword, not `schema`.** Trying to use a `schema` as a
   mixin produces `E2D34 IllegalInheritError: illegal schema mixin
   object type, expected mixin, got 'AuditLog'`.

2. **Mixin name must end with `Mixin`.** Trying to declare
   `mixin AuditLog: ...` produces `E1001 NameError: a valid mixin
   name should end with 'Mixin', got 'AuditLog'`. The convention is
   load-bearing; codegen has to respect it when generating Rust type
   names (strip the `Mixin` suffix? keep it? — see "Codegen naming"
   below).

The resulting `SchemaValue` for `foo`:

- `name == "Foo"` (the consuming schema's name; the mixin name does
  *not* appear).
- Field reachability: `foo.dict_get_value("audit_id") == "abc123"`,
  `foo.dict_get_value("created_by") == "system"` (default flowed
  through), `foo.dict_get_value("name") == "alpha"`. All three fields
  are inlined into the same dict.

In other words, **KCL mixins are flattened at the value-tree level**.
The mixin's name and identity vanish; what remains is a schema whose
field set is the union of its own + mixed-in fields.

### Codegen consequence

For the **result-tree path** (the `TryFrom<&ValueRef>` consumer side):
codegen does not need to emit a separate Rust type for the mixin. The
consuming schema's generated struct has all the mixed-in fields
appearing alongside its own. Sema's resolved type representation is
expected to expose the inlined view directly (verified during step
3's IR extraction); if it doesn't and codegen has to perform the
flattening itself, the work is mechanical.

For the **schema-definition path** (Rust code that needs to talk
*about* the mixin as a concept — e.g. a generic function over any
schema that has audit fields): the inline-into-consuming-schema
strategy loses this affordance. The plan's D9 trait-per-parent
pattern (deferred from Phase 4 MVP) would address it. Decision:
**not in scope for Phase 4 MVP** since no consumer in flight needs
the trait. If a future consumer materialises (Tilley audit logging?),
add `Phase 4B: mixin-as-trait codegen` and emit `trait AuditLogMixin`
+ `impl AuditLogMixin for Foo` alongside the existing inline shape.

### Codegen naming

Mixin names canonically end with `Mixin`. Two choices for the
generated Rust type if/when we add trait-per-mixin codegen (Phase 4B,
not now):

- Strip the suffix: `trait AuditLog`. Mirrors how Rust traits don't
  usually carry a "Mixin" suffix; reads naturally in `impl AuditLog
  for Foo` bounds.
- Keep the suffix: `trait AuditLogMixin`. Lossless round-trip to the
  KCL source; explicit about the origin.

**Tentative preference: keep the suffix.** Preserves source identity,
avoids the "what if there's a schema *also* named AuditLog?" collision.
Final answer goes in Phase 4B's plan when it materialises.

### What Phase 4 MVP does

- Codegen against a `mixin` declaration: treated as transparent. No
  Rust type emitted for the mixin itself.
- Codegen against a `schema X: mixin [YMixin]; ...` declaration: emit
  a struct `X` with fields from both `X` and `YMixin`. Sema is
  expected to provide the inlined field list; if it does not, the
  codegen IR construction does the flattening.
- The Phase 4 step 6 end-to-end test exercises the mixin case
  alongside the discriminator + optionals + lists cases.

---

## Other observations during the probe

- The runtime's error messages on misuse (`E2D34`, `E1001`) are
  actionable. Codegen-time errors should aim for the same quality:
  point at the offending file:line, name the constraint that was
  violated, suggest the fix. Sema already produces structured
  diagnostics that codegen can surface.
- The mixin probe test (`test_structured_value_mixin_probe`) prints
  the runtime-reported schema name via `eprintln!` to make
  unexpected runtime drift surface in test logs. If a future KCL
  upgrade changes what `SchemaValue.name` reports for a
  mixin-using schema, the test fails with a useful diagnostic.

---

## Implementation order for Phase 4 (post-memo)

1. **Step 2: crate skeleton.** `crates/rust-codegen/` with the
   public API (`generate_to_string`, `generate_to_file`) and an
   internal `SchemaIR` data model.
2. **Step 3: IR extraction.** Walk `kcl_sema`'s resolved Type
   representation; produce `SchemaIR` per schema. Inheritance
   (D9) and unit-typed values are codegen-time errors per Phase 4
   MVP scope. Mixins are transparent (this memo).
3. **Step 4: Rust emitter.** Primitives, lists, dicts, schemas,
   optionals (per D5 mapping above). `TryFrom<&ValueRef>` impl with
   the seatbelt rustdoc from D4.
4. **Step 5: Unions.** D6's three flavours: disjoint structural,
   discriminator-required, primitive-structural. Plus the
   single-schema-with-discriminator annotation
   `# @rust: tagged_enum(discriminator = "type")` for Tilley's
   TestAssertion case.
5. **Step 6: End-to-end test against the Tilley-shaped fixture.**
   Same KCL as `test_structured_value_tilley_shaped_fixture`; run
   codegen, build the generated types, evaluate the program via
   `kcl-embed`, `try_into()` into the generated types, assert
   structural equality with hand-written expected values. The
   proof that the foundation works.
