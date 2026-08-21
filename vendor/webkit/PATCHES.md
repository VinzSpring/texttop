# Vendored `webkit` 0.3.11 — local patches

Upstream: <https://crates.io/crates/webkit> version 0.3.11 (MIT OR Apache-2.0).

This is a verbatim copy of the published crate plus the changes below. It is
vendored rather than referenced by version because `src/engine/mac.rs` depends
on API that does not exist upstream, and because the published crate does not
compile against the Xcode 15 / macOS 14 SDK.

Regenerate a diff against pristine upstream with:

```sh
cargo download webkit==0.3.11   # or: tar xzf ~/.cargo/registry/cache/*/webkit-0.3.11.crate
diff -ru webkit-0.3.11 vendor/webkit --exclude=Cargo.lock --exclude=Cargo.toml.orig
```

## 1. New API that `src/engine/mac.rs` requires

| Symbol | Files |
| --- | --- |
| `WebView::set_frame_size(w, h)` | `src/webview.rs`, `src/ffi/mod.rs`, `swift-bridge/.../WKWebView.swift` |
| `WebView::take_snapshot_rgba(pixel_width)` | `src/webview.rs`, `src/ffi/mod.rs`, `swift-bridge/.../WKWebView.swift` |
| `webkit::drain_main()` | `src/lib.rs`, `src/ffi/mod.rs`, `swift-bridge/.../Core.swift` |
| `WebView::attach_window(w, h, mode, alpha)` | `src/webview.rs`, `src/ffi/mod.rs`, `swift-bridge/.../WKWebView.swift` |

`set_frame_size` resizes the off-screen `WKWebView` so the page lays out at the
real TUI viewport size. Upstream only ever creates the view at a fixed 800×600.

`attach_window` hosts the view in a real `NSWindow`. Upstream only ever
creates a window-less view, which WebKit treats as a background tab:
`requestAnimationFrame` is never serviced and `<video>` never decodes
(`videoWidth` stays 0), so pages capture frozen. `mode` 0 parks the window
off-screen — the Windows engine's trick, which macOS occlusion detection
defeats — and `mode` 1 places it on-screen above other windows at a given
alpha. At alpha 0 it is invisible and click-through, and the page behaves
like a normal tab.

`take_snapshot_rgba` returns raw premultiplied-RGBA pixels scaled to an exact
pixel width, instead of upstream's `take_snapshot_png`. This skips a PNG
encode + decode per frame and normalizes away the Retina backing scale.
The buffer is tightly packed (`bytesPerRow == width * 4`) and top-down.

`drain_main` is currently unused by texttop (`main.rs` pumps the run loop via
the upstream `pump_run_loop`), but is kept because it is the non-blocking
variant and is cheap to carry.

## 2. Upstream bug fix: nil `request` traps

`WKFrameInfo.request` and `WKNavigationAction.request` are declared non-null in
the SDK headers, but WebKit hands back `nil` for `loadHTMLString` navigations.
Force-bridging that to `URLRequest` traps and kills the process. texttop uses
`load_html` for every built-in page, so upstream crashes on startup.

Fixed by reading the property via KVC (`wkSafeRequest` in `Core.swift`) so a
nil stays nil. Applied in `WKNavigationDelegate.swift`,
`WKScriptMessageHandler.swift`, and `WKUIDelegate.swift`.

## 3. SDK availability stubs (Xcode 15 / macOS 14)

These upstream call sites reference symbols absent from the macOS 14 SDK, so
they fail to compile rather than degrading at runtime. Each is replaced with an
explicit unsupported result:

- `WKHTTPCookieStore.setCookies(_:)` — macOS 26.0+
- `WKNavigationAction.isContentRuleListRedirect` — macOS 26.0+
- `WKDownload.isUserInitiated` — macOS 15.2+
- `WKWebsiteDataStore` / `WKWebView` `fetchData` family — macOS 26.0+
- `WKWebExtension` / `WKWebViewConfiguration` availability gaps

None of these are used by texttop. Building with a newer SDK would let all of
them be restored to the upstream implementation.

## 4. Examples

`examples/98_texttop_verify.rs` asserts the invariants `src/engine/mac.rs`
relies on — viewport parity, tight pixel packing, top-down row order, and the
`frame_px == css_px * zoom` mapping used for mouse input and the crisp-text
overlay. Run it after touching either side:

```sh
cargo run --release --example 98_texttop_verify --manifest-path vendor/webkit/Cargo.toml
```

`examples/99_resize_check.rs` is a smaller timing/lay-out sanity check.

`examples/94_window_modes.rs` compares the window placements above on a real
video page, reporting visibility state, whether rAF fires, whether `<video>`
decodes, and how many snapshots differ. This is how `attach_window`'s mode and
alpha were chosen.

`examples/95_snap_bench.rs` measures capture cost against the layout viewport.
It is what established that the cost is the viewport, not the snapshot
resolution: with a video playing, a 1712x1040 viewport takes ~81ms per capture
and stays ~75ms even when the snapshot is scaled to 428px wide, while 856x520
takes ~30ms. `BENCH_WINDOW=1` attaches a window so media actually plays —
without it every frame is identical and the numbers are meaningless.

`examples/97_scroll_check.rs` checks that `window.scrollBy` moves the page
*and* that the snapshot follows, and compares the alternative scroll targets
(`documentElement` vs `scrollingElement`).

`examples/96_page_diag.rs` loads a real URL at a given viewport and dumps the
metrics that explain a blank or unscrollable page — visibility state, body
height, scroll height, image and text counts, whether `IntersectionObserver`
and `requestAnimationFrame` fire, and how much of the snapshot is actually
painted. `DIAG_VISIBLE=1` installs the same visibility shim the app injects
(see `VISIBILITY_JS` in `src/engine/mod.rs`), and `DIAG_CAPTURE=1` reproduces
the app's 30fps snapshot loop. This is how that fix was verified:

```sh
cargo run --release --example 96_page_diag --manifest-path vendor/webkit/Cargo.toml \
  -- https://www.youtube.com/
DIAG_VISIBLE=1 cargo run --release --example 96_page_diag \
  --manifest-path vendor/webkit/Cargo.toml -- https://www.youtube.com/
```
