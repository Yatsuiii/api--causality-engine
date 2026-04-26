# Assertions

Three assertion forms are supported on every step:

```yaml
assert:
  - status: 200                        # exact match, or { eq: 200, lt: 400, ... }
  - body:
      data.user.id: { exists: true }   # JSONPath dot-notation
      data.user.role: { eq: "admin" }
  - schema:                            # JSONSchema validation (P0.2)
      openapi: ./specs/api.json
      component: User
      strict: true
```

Schema assertions accept three shapes (untagged enum):

| Form | YAML |
|---|---|
| Inline JSONSchema | `schema: { type: object, required: [...], properties: {...} }` |
| File path (JSON or YAML) | `schema: ./schemas/user.json` |
| OpenAPI component reference | `schema: { openapi: ./api.json, component: User, strict: true }` |

`strict: true` injects `additionalProperties: false` at every object node before
compilation. This is what makes drift detection work for "field appeared that
the spec didn't predict" — without it, a backend adding new fields silently
passes the old contract.

## Caching

Compiled schemas are cached per scenario run, keyed by:

- Inline: canonical-key-order JSON of the schema value
- File: resolved absolute path
- OpenAPI: `{path, component, strict|lax}` triple — strictness is part of the
  key so two assertions on the same component with different strict values
  never share a compiled schema

## Known limitations

- **OpenAPI 3.0 vs 3.1 draft selection.** The `jsonschema` crate uses its
  default draft (Draft 2020-12) regardless of the `openapi:` version field in
  the spec. OpenAPI 3.0 specs technically use a Draft-04ish dialect. In
  practice, the validator works for the cases drift detection cares about
  (extra properties, missing required fields, type mismatches). If you hit a
  spec that depends on draft-specific keyword behavior (e.g. `nullable:` in
  OAS 3.0 vs `type: ["string", "null"]` in 3.1), the validation result may
  differ from what the spec author intended. Workarounds: pre-process the
  spec, or use inline JSONSchema directly.

- **Cyclic `$ref` in OpenAPI components.** Inlining cycles indefinitely would
  hang. The resolver inlines one level, then rewrites the inner `$ref` from
  `#/components/schemas/X` to `urn:ace:openapi-root#/components/schemas/X` and
  registers the original OpenAPI doc under that synthetic URI at compile
  time. This makes deep recursion validate correctly (covered by
  `cyclic_openapi_component_compiles_and_validates_deep_ref`). Caveat: only
  the first inlined hop has `strict: true` injection applied; deeper hops
  validate against the original (lax) component, so strict mode is "leaky"
  for cyclic schemas. If you need strict-everywhere on a recursive type,
  flatten the schema or use inline JSONSchema directly.

- **JSONSchema error messages.** Validation failures pass through the
  `jsonschema` crate's English: e.g. `Additional properties are not allowed
  ('discounts' was unexpected) at /`. Human-friendly reformatting (e.g.
  `+ unexpected field: discounts`) is tracked in
  [P0.4 — Output clarity](improvement-plan/p0-4-output-clarity.md).

## Schema feature

JSONSchema validation is a default feature of the `engine` crate (`schema`).
It pulls `jsonschema` (~1.5 MB to release binary) and `serde_yaml` for
loading YAML schema files. Disabling the feature reduces binary size at the
cost of `schema:` assertions emitting a "feature disabled" failure at
runtime instead of validating.
