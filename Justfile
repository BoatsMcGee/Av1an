precommit:
    cargo +nightly fmt
    cargo clippy --profile ci --tests --benches -- -D warnings
    cargo test --profile ci -p andean-condor
    cargo test --profile ci -p california-condor