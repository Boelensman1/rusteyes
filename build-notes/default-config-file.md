# default-config-file

## Goal

- Write an editable default config file on startup when the implicit config path
  is missing.

## Changes

- Missing implicit config files now create parent directories and write a
  default YAML file before continuing with typed defaults.
- Explicit `RUSTEYES_CONFIG` paths remain strict: a missing explicit path still
  reports a read error and does not create a file.
- Enabled `serde-saphyr` serialization support and generate the default YAML
  from `Config::default()` through a private serializable view instead of
  maintaining a parallel YAML literal.
- Default file creation uses `create_new` so an existing config is not
  overwritten if another process creates it between the read and write.

## Decisions

- Keep config structs as the source of truth for default values.
- Treat default-file write failures as startup errors so the user sees the
  unusable config path instead of silently running without an editable file.

## Commands

- `nix develop --command cargo test --lib config::tests`
- `make check`

## Follow-up

- None.
