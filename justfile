build:
    cargo build --release

rel *ARGS:
    cargo release {{ ARGS }}
