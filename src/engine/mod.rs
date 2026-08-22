//! Embedded web engine abstraction. Each platform provides an `Engine` with
//! the same surface: WebView2 (Edge) on Windows, WebKitGTK on Linux,
//! WKWebView on macOS, and a stub elsewhere / when the `web-engine` feature
//! is off.

pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<u8>,
    /// Monotonic capture number — the reliable identity for caching and
    /// fps counting (frame allocations reuse addresses, pointers do not work).
    pub seq: u64,
}

/// Global frame sequence shared by all engine instances.
pub(crate) static FRAME_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Bumped on every navigation so in-flight captures from the previous
/// document are discarded instead of painting stale pixels.
pub(crate) static PAGE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn next_seq() -> u64 {
    FRAME_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn page_gen() -> u64 {
    PAGE_GEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn bump_page_gen() -> u64 {
    PAGE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

#[derive(Clone, Default)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

#[derive(Clone)]
pub enum Status {
    Starting,
    Ready,
    Error(String),
}

/// Kinds of stored site data the engine can drop.
#[derive(Clone, Copy, PartialEq)]
pub enum DataKind {
    Cookies,
    Cache,
    /// Everything WebKit keeps for a site: cookies, caches, local storage,
    /// IndexedDB, service workers.
    All,
}

/// One stored cookie, for the browser's cookie inspector. Deliberately no
/// value — showing session tokens on screen buys nothing and leaks plenty.
#[derive(Clone)]
pub struct CookieInfo {
    pub domain: String,
    pub name: String,
    pub secure: bool,
    pub http_only: bool,
    pub session_only: bool,
}

#[derive(Clone, Copy)]
pub enum MouseKind {
    Move,
    Drag,
    Down,
    Up,
}

pub enum Cmd {
    Navigate(String),
    NavigateHtml(String),
    Back,
    Forward,
    Reload,
    ResizePx(i32, i32),
    Zoom(f64),
    Script(String),
    /// Pointer event at physical viewport pixel coordinates.
    Mouse { kind: MouseKind, x: i32, y: i32 },
    /// Enable/disable the crisp text overlay: when on, the page's text is
    /// made transparent in the pixel frame and extracted as positioned
    /// words so the TUI can draw it with real terminal characters.
    TextOverlay(bool),
    /// Drop stored site data of this kind.
    ClearData(DataKind),
    /// Whether to accept cookies from here on. Existing ones are untouched —
    /// pair it with [`Cmd::ClearData`] to start clean.
    SetCookiePolicy(bool),
    /// Re-read the cookie store into [`Engine::cookies`].
    RefreshCookies,
}

/// One extracted word (CSS pixel coordinates).
#[derive(Clone)]
pub struct TextRun {
    pub x: f32,
    pub y: f32,
    #[allow(dead_code)]
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub color: (u8, u8, u8),
    pub bold: bool,
}

/// In-page extraction script. Returns JSON (stringified) of
/// [x, y, w, h, word, r, g, b, bold] rows, and injects a style that hides
/// rendered text from screenshots (the TUI redraws it crisply).
pub const EXTRACT_JS: &str = r##"(function(){
 try{
  // Extract first; only hide pixel text once we have words to redraw.
  // Hiding up-front left pages blank while the DOM was still loading.
  var out=[];var vw=innerWidth,vh=innerHeight;var count=0;
  function push(r,txt,cs){
    if(!r||r.width<=0||r.height<=0)return;
    if(r.bottom<0||r.top>vh||r.right<0||r.left>vw)return;
    var c=(cs.color.match(/\d+/g)||[0,0,0]);
    var b=(parseInt(cs.fontWeight)||400)>=600?1:0;
    out.push([Math.round(r.left),Math.round(r.top),Math.round(r.width),Math.round(r.height),txt,+c[0],+c[1],+c[2],b]);
    count++;
  }
  if(document.body){
    var walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,null);
    var n;
    while((n=walker.nextNode())&&count<3000){
      var full=n.textContent;if(!full||!full.trim())continue;
      var el=n.parentElement;if(!el)continue;
      var cs=getComputedStyle(el);
      if(cs.display=='none'||cs.visibility=='hidden')continue;
      var pr=el.getBoundingClientRect();
      if(pr.bottom<0||pr.top>vh||pr.width==0)continue;
      var range=document.createRange();
      var re=/\S+/g,m;
      while((m=re.exec(full))&&count<3000){
        range.setStart(n,m.index);range.setEnd(n,m.index+m[0].length);
        push(range.getBoundingClientRect(),m[0],cs);
      }
    }
    var inputs=document.querySelectorAll('input,textarea');
    for(var i=0;i<inputs.length&&count<3200;i++){
      var e2=inputs[i];
      var v=e2.type==='password'?'•'.repeat((e2.value||'').length):(e2.value||e2.placeholder||'');
      if(!v)continue;
      push(e2.getBoundingClientRect(),v.substring(0,200),getComputedStyle(e2));
    }
  }
  var st=document.getElementById('__ttp_style');
  if(count>0){
    if(!st){st=document.createElement('style');st.id='__ttp_style';
      st.textContent='*{-webkit-text-fill-color:transparent !important;caret-color:transparent !important;text-shadow:none !important;}';
      document.documentElement.appendChild(st);}
  }else if(st){st.remove();}
  return JSON.stringify(out);
 }catch(e){return '[]';}
})()"##;

/// Undo the screenshot-text-hiding style (overlay off).
pub const REMOVE_STYLE_JS: &str =
    "(function(){var st=document.getElementById('__ttp_style');if(st)st.remove();})()";

/// Convince a page that it is on screen. Must run *before* the page's own
/// scripts, so it is installed as a document-start user script.
///
/// Every engine here renders off-screen, so the view is never in a visible
/// window and the Page Visibility API reports `hidden`. Single-page apps
/// treat that as "user switched tabs" and skip layout and data fetching:
/// youtube.com collapses `body` to zero height, which renders as a blank
/// page *and* leaves nothing to scroll, since the document is then exactly
/// one viewport tall. Overriding the getters takes youtube.com's body from
/// 0px to ~3000px and restores scrolling.
///
/// Known limit, and two things that do *not* fix it: on macOS an off-screen
/// WKWebView never services `requestAnimationFrame`, because it has no
/// display link. Frameworks that commit rendering in a rAF callback — YouTube's
/// Polymer among them — therefore lay out their skeleton and stop, which is
/// why its video grid stays a set of empty cards even though the feed JSON
/// arrived (`ytInitialData` is ~110KB and no script errors are raised).
/// Windows does not have this problem because WebView2 renders into a real
/// visible HWND parked off-screen, so it keeps a live compositor.
///
/// Tried and rejected: hosting the WKWebView in a real NSWindow parked at
/// -30000,-30000 changes nothing, since macOS occlusion detection reports a
/// window outside every screen as not visible. Replacing `requestAnimationFrame`
/// with a `setTimeout` shim makes things distinctly *worse* — YouTube then
/// takes a frame-timed path that collapses the layout again, dropping the
/// painted area from 59% back to 2%. See `vendor/webkit/examples/96_page_diag.rs`.
pub const VISIBILITY_JS: &str = r#"(function(){
 try{
  var d=Document.prototype;
  Object.defineProperty(d,'visibilityState',{get:function(){return 'visible';},configurable:true});
  Object.defineProperty(d,'webkitVisibilityState',{get:function(){return 'visible';},configurable:true});
  Object.defineProperty(d,'hidden',{get:function(){return false;},configurable:true});
  Object.defineProperty(d,'webkitHidden',{get:function(){return false;},configurable:true});
  d.hasFocus=function(){return true;};
  // Some apps only re-check on the event, not on first paint.
  addEventListener('DOMContentLoaded',function(){
    document.dispatchEvent(new Event('visibilitychange'));
  });
 }catch(e){}
})()"#;

/// Simulated pointer events for engines without a native input path
/// (WebKitGTK, or WebView2 when its input window can't be found). Tracks
/// the hovered element so JS-driven hover UIs (menus, tooltips) fire
/// correct mouseover/mouseout chains. Coordinates are CSS pixels.
pub fn mouse_sim_js(kind: MouseKind, x: i32, y: i32) -> String {
    let action = match kind {
        MouseKind::Move | MouseKind::Drag => "fire('mousemove');",
        MouseKind::Down => {
            "fire('mousedown');\
             if(el.tagName=='INPUT'||el.tagName=='TEXTAREA'||el.isContentEditable){el.focus();}"
        }
        MouseKind::Up => {
            "fire('mouseup');\
             var t=el.closest?el.closest('a,button,[role=button],input,select,summary,label,video'):null;\
             if(el.tagName=='INPUT'||el.tagName=='TEXTAREA'||el.isContentEditable){el.focus();if(el.click)el.click();}\
             else if(t){if(t.tagName=='VIDEO'){if(t.paused)t.play();else t.pause();}else{t.click();}}\
             else{fire('click');}"
        }
    };
    format!(
        "(function(x,y){{var el=document.elementFromPoint(x,y)||document.body;\
         function fire(t){{el.dispatchEvent(new MouseEvent(t,{{bubbles:true,cancelable:true,clientX:x,clientY:y,view:window}}));}}\
         var prev=window.__ttp_hover;\
         if(prev!==el){{\
           if(prev)prev.dispatchEvent(new MouseEvent('mouseout',{{bubbles:true,clientX:x,clientY:y}}));\
           el.dispatchEvent(new MouseEvent('mouseover',{{bubbles:true,clientX:x,clientY:y}}));\
           window.__ttp_hover=el;}}\
         {action}}})({x},{y});"
    )
}

/// Parse the extraction script's JSON payload into text runs.
pub fn parse_text_runs(json: &str) -> Vec<TextRun> {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for row in arr {
            let a = match row.as_array() {
                Some(a) if a.len() >= 9 => a,
                _ => continue,
            };
            let f = |i: usize| a[i].as_f64().unwrap_or(0.0) as f32;
            let u = |i: usize| a[i].as_u64().unwrap_or(0).min(255) as u8;
            let text = a[4].as_str().unwrap_or("").to_string();
            if text.is_empty() {
                continue;
            }
            out.push(TextRun {
                x: f(0),
                y: f(1),
                w: f(2),
                h: f(3),
                text,
                color: (u(5), u(6), u(7)),
                bold: a[8].as_u64().unwrap_or(0) == 1,
            });
        }
    }
    out
}

/// Drop the shared page surface so the TUI can't keep painting a previous
/// document's pixels / crisp-text runs after a navigation.
pub(crate) fn clear_page_state(
    frame: &std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<Frame>>>>,
    texts: &std::sync::Arc<std::sync::Mutex<Vec<TextRun>>>,
) {
    bump_page_gen();
    *frame.lock().unwrap() = None;
    texts.lock().unwrap().clear();
}

pub fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(all(windows, feature = "web-engine"))]
#[path = "win.rs"]
mod imp;

#[cfg(all(target_os = "linux", feature = "web-engine"))]
#[path = "linux.rs"]
mod imp;

#[cfg(all(target_os = "macos", feature = "web-engine"))]
#[path = "mac.rs"]
mod imp;

#[cfg(not(any(
    all(windows, feature = "web-engine"),
    all(target_os = "linux", feature = "web-engine"),
    all(target_os = "macos", feature = "web-engine")
)))]
#[path = "stub.rs"]
mod imp;

pub use imp::Engine;
