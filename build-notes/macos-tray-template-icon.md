# macos-tray-template-icon

## Goal

Make the macOS menu-bar tray icon look native: a monochrome **template image**
that the system tints to the menu-bar text colour (white on a dark menu bar,
black on a light one) instead of the full-colour gear.

## Change

- `src/ui.rs`, `build_tray_icon`: opt the tray icon into macOS template
  rendering via `TrayIconBuilder::with_icon_as_template(true)`, gated behind
  `#[cfg(target_os = "macos")]` using a shadowed `builder` binding so the chain
  stays single-expression and Linux is byte-for-byte unchanged.

```rust
let builder = TrayIconBuilder::new()
    .with_tooltip(TOOLTIP)
    .with_icon(icon)
    .with_menu(Box::new(menu))
    .with_menu_on_left_click(true);

#[cfg(target_os = "macos")]
let builder = builder.with_icon_as_template(true);

let tray_icon = builder.build()...;
```

## Decisions / tradeoffs

- A macOS template image ignores the icon's RGB and masks by its alpha
  silhouette, so the existing embedded asset (`package/icons/rusteyes-tray.rgba`,
  64x64) is reused unchanged — no new icon was generated.
- Scope is macOS only. `with_icon_as_template` is a no-op on the Linux/gtk
  backend (the attribute is stored but unused), and we gate it anyway so the
  Linux system tray keeps the full-colour gear (a forced-white icon could be
  invisible on light panel themes).
- Adaptive (template) over forced-white: an always-white icon disappears on a
  Light-mode menu bar; the template inverts automatically, matching native apps.
- Shadowing instead of `let mut builder` avoids an `unused_mut` warning on
  non-macOS targets (clippy runs with `-D warnings`).
- `package/icons/rusteyes-white.png` (a pure-white recolour prepared while
  exploring this) is **not used** — a template ignores RGB — so it and its
  `._rusteyes-white.png` AppleDouble sidecar were deleted.

## Verification

- `make check` passes: `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -D warnings`, and 261 tests (incl.
  `tray_icon_asset_matches_declared_size`) all green on macOS, where the
  `cfg(target_os = "macos")` branch is compiled.
- Manual macOS check still pending: `make run`, confirm the menu-bar gear is
  monochrome and inverts when toggling System Settings -> Appearance between
  Light and Dark.

## Follow-up

- None outstanding beyond the manual macOS appearance check above.
