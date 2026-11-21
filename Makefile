.PHONY: run run-wsl build-win run-win test check clippy build

# WSL 環境なら自動で X11 + ソフトウェアGL フラグを付与して起動する
run:
ifdef WSL_DISTRO_NAME
	WAYLAND_DISPLAY="" WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1 cargo run --bin conductty
else
	cargo run --bin conductty
endif

run-wsl:
	WAYLAND_DISPLAY="" WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1 cargo run --bin conductty

build-win:
	cargo build --target x86_64-pc-windows-gnu --bin conductty
	cp -f target/x86_64-pc-windows-gnu/debug/build/webview2-com-sys-*/out/x64/WebView2Loader.dll target/x86_64-pc-windows-gnu/debug/

run-win: build-win
	./target/x86_64-pc-windows-gnu/debug/conductty.exe

test:
	cargo test --workspace

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace

build:
	cargo build --release --bin conductty
