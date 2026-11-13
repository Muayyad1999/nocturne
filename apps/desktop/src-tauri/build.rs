use std::path::Path;

fn main() {
    // Tauri insists the interface exists before it will build. It is usually
    // built first (`pnpm --dir ui build`), but a Rust-only check should not
    // fail for want of it: leave a page that says what to do instead.
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../ui/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            &index,
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>Nocturne</title><style>html{background:#090d18;color:#e7ebf3;\
             font:14px/1.5 system-ui,sans-serif}body{margin:0;display:grid;place-items:center;\
             height:100vh}p{max-width:34rem;text-align:center}code{color:#9db4ff}</style></head>\
             <body><p>The Nocturne interface was not built into this copy.<br>\
             Build it with <code>pnpm --dir ui build</code> and start Nocturne again.</p></body></html>",
        );
    }
    println!("cargo:rerun-if-changed=../../../ui/dist/index.html");
    tauri_build::build();
}
