.PHONY: build run serve test clean release release-musl docker docker-run fmt clippy check

build:
	cargo build

run:
	cargo run --bin openintent -- run

serve:
	cargo run --bin openintent -- serve

test:
	cargo test --workspace

clean:
	cargo clean

release:
	cargo build --release

release-musl:
	cargo build --release --target x86_64-unknown-linux-musl

docker:
	docker build -t openintentos .

docker-run:
	docker compose up -d

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace -- -D warnings

check: fmt clippy test
