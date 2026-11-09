precommit:
    cargo +stable fmt --all
    cargo +stable clippy --profile ci --workspace --tests --benches -- -D warnings
    cargo +stable test --profile ci --workspace
    cargo +stable test --profile ci -p av-metrics-vmaf --no-default-features --features cuda

# Manual gate on a machine with libvship and a supported GPU, not a CPU CI runner.
test-gpu:
    AV_METRICS_REQUIRE_GPU=1 cargo +stable test --profile ci -p av-metrics-vship -- --nocapture
