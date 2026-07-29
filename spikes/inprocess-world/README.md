# spike: in-process typst backend

Not part of the workspace. This is the throwaway that resolved P5 — it is kept
because the numbers in `docs/findings.md` came from it and because the `World`
in `src/world.rs` is the starting point for `lilook-compile` (plan-1.0 M1).

```sh
cargo run --release            -- 5000    # cold / warm / probe / render timings
cargo run --release --bin series -- 5000  # evaluated series-data recovery
```

`world.rs` is `include!`d rather than being a module, which is a spike-shaped
thing to do; it becomes a real crate in M1.
