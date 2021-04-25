default: docs

docs: Cargo.toml Cargo.lock src/*.rs
	cargo rustdoc --open -- --html-in-header rustdoc-include-katex-header.html

