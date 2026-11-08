precommit:
    cargo +stable fmt --all
    cargo +stable clippy --profile ci --tests --benches -- -D warnings
    cargo +stable test --profile ci -p andean-condor
    cargo +stable test --profile ci -p california-condor
