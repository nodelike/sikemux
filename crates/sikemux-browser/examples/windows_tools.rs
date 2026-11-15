//! Drives a WebView2 tab in a plain window through the agents' own browser tools: navigate,
//! state, click, type, evaluate and screenshot, with no app and no core. Each answer is
//! written to the folder named first, with the screenshot beside it, and `passed` once all
//! of them answered; the window stays open to be pictured. Windows only.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    proof::run();
}

#[cfg(windows)]
mod proof {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use base64::Engine;
    use serde_json::{json, Value};
    use sikemux_base::cli::protocol::HarnessRequest;
    use sikemux_browser::{
        run_pages_on_this_thread, runtime_version, tools, BrowserBounds, BrowserDownload,
        BrowserManager, BrowserShortcut, Host, ParentView,
    };
    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
        RegisterClassW, TranslateMessage, CW_USEDEFAULT, MSG, WINDOW_EX_STYLE, WM_DESTROY,
        WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    const AGENT: &str = "proof";
    const WIDTH: f64 = 1000.0;
    const HEIGHT: f64 = 680.0;
    const PAGE: &str = r#"<!doctype html><title>Sikemux on WebView2</title>
<style>body{font:16px system-ui;background:#0b0b0d;color:#e6e6e9;margin:40px}
input,button{font:inherit;padding:8px 12px}button{position:absolute;left:40px;top:200px;width:160px;height:40px}</style>
<h1>A page in WebView2</h1><p>Driven by the agents' browser tools.</p>
<input id="name" placeholder="type here">
<button id="go" onclick="document.title='clicked';this.textContent='Clicked'">Click me</button>"#;

    struct ProofHost {
        window: isize,
        folder: PathBuf,
    }

    impl Host for ProofHost {
        fn window(&self) -> Result<ParentView, String> {
            Ok(ParentView {
                view: self.window as *mut _,
                beneath: false,
                ground: Some([11, 11, 13]),
            })
        }
        fn state_dir(&self) -> Option<PathBuf> {
            Some(self.folder.join("state"))
        }
        fn cache_dir(&self) -> Option<PathBuf> {
            Some(self.folder.join("cache"))
        }
        fn videos_dir(&self) -> Option<PathBuf> {
            Some(self.folder.clone())
        }
        fn page_url(&self, _agent_id: &str, _html: &str) -> Result<String, String> {
            Err("no pages here".into())
        }
        fn tabs_changed(&self) {}
        fn downloaded(&self, _download: &BrowserDownload) {}
        fn acting(&self, _agent_id: &str) {}
        fn shortcut(&self, _shortcut: BrowserShortcut) {}
    }

    pub fn run() {
        let folder = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "proof".into()));
        std::fs::create_dir_all(&folder).expect("the folder for answers can be made");
        match runtime_version() {
            Ok(version) => println!("WebView2 {version}"),
            Err(why) => {
                println!("{why}");
                std::process::exit(2);
            }
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Runtime::new().expect("the tools' runtime starts");
        sikemux_base::runtime::set(runtime.handle().clone());
        let window = open_window();
        run_pages_on_this_thread();
        let host = Arc::new(ProofHost {
            window,
            folder: folder.clone(),
        });
        std::thread::spawn(move || {
            let manager = BrowserManager::new(host);
            if !drive(&manager, &folder) {
                std::process::exit(1);
            }
            let _ = std::fs::write(folder.join("passed"), "");
            // The tab closes with its manager, so both stay until the window is pictured.
            loop {
                std::thread::park();
            }
        });
        let mut message = MSG::default();
        // SAFETY: the window's own message loop, on the thread that made it.
        unsafe {
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    fn open_window() -> isize {
        // SAFETY: a plain top-level window of our own class, on this thread.
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).expect("the program's module").0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_events),
                hInstance: instance,
                lpszClassName: w!("SikemuxPageProof"),
                ..Default::default()
            };
            RegisterClassW(&class);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("SikemuxPageProof"),
                w!("Sikemux page on WebView2"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                (WIDTH + 16.0) as i32,
                (HEIGHT + 39.0) as i32,
                None,
                None,
                Some(instance),
                None,
            )
            .expect("the window opens")
            .0 as isize
        }
    }

    unsafe extern "system" fn window_events(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DESTROY {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        // SAFETY: the system's default handling of a message to our own window.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    fn drive(manager: &BrowserManager, folder: &Path) -> bool {
        let page = folder.join("page.html");
        std::fs::write(&page, PAGE).expect("the page is written");
        let url = match manager.local_file_url(&page) {
            Ok(url) => url,
            Err(why) => return failed("local_file_url", &why),
        };
        let steps = [
            ("browser.navigate", json!({ "url": url })),
            ("show", Value::Null),
            ("browser.state", json!({})),
            ("browser.click", json!({ "x": 120, "y": 220 })),
            (
                "browser.type",
                json!({ "selector": "#name", "text": "typed by an agent" }),
            ),
            (
                "browser.evaluate",
                json!({ "script": "return document.title + ' / ' + document.querySelector('#name').value" }),
            ),
            ("browser.screenshot", json!({})),
        ];
        let mut passed = true;
        for (number, (method, params)) in steps.into_iter().enumerate() {
            if method == "show" {
                let shown = manager.set_bounds(AGENT, Some(bounds()));
                println!("show: {shown:?}");
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let request = HarnessRequest {
                id: number.to_string(),
                project: String::new(),
                agent_id: Some(AGENT.into()),
                method: method.into(),
                params,
            };
            let answer = tools::execute(manager, &request, &|_, _| Box::pin(async { Ok(()) }));
            let name = method.trim_start_matches("browser.");
            match answer {
                Ok(mut answer) => {
                    if let Some(data) = answer.get("data").and_then(Value::as_str) {
                        let jpeg = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap_or_default();
                        let _ = std::fs::write(folder.join("screenshot.jpg"), jpeg);
                        answer["data"] = json!("written to screenshot.jpg");
                    }
                    let text = serde_json::to_string_pretty(&answer).unwrap_or_default();
                    println!("{name}: {}", text.chars().take(600).collect::<String>());
                    let _ = std::fs::write(folder.join(format!("{number}-{name}.json")), text);
                }
                Err(why) => passed = failed(name, &why),
            }
        }
        passed
    }

    fn bounds() -> BrowserBounds {
        BrowserBounds {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
            clip_left: 0.0,
            clip_right: 0.0,
            holes: Vec::new(),
            dim: 0.0,
            opacity: 1.0,
            bottom_radius: 0.0,
        }
    }

    fn failed(step: &str, why: &str) -> bool {
        println!("{step} failed: {why}");
        false
    }
}
