use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Annotate a test function so it prints its wall-clock duration and
/// fails if that duration exceeds a soft regression threshold.
///
/// **Intent**: catch O(n²)-shaped or accidentally-quadratic regressions
/// in LSP request handlers. The wall-clock assertion is a sanity
/// check — "this hover used to take ~100ms; if it ever takes seconds,
/// something is wrong" — *not* a benchmark in the strict sense.
///
/// **Threshold**: 5 seconds. Originally 400ms, which was tight enough
/// to be timing-flaky: under `cargo test --workspace` the compile phase
/// of a workspace-dep that's added by *any* PR (e.g. tracing-subscriber
/// in Phase 6b) can interleave with running test binaries via cargo's
/// job server, pushing wall-clock test runtimes from ~100ms in
/// isolation to ~500ms under contention. 5s preserves the
/// catch-the-pathological-case intent without producing CI noise on
/// every unrelated workspace dependency change.
///
/// If a future contributor wants tighter timing assertions, the right
/// move is a dedicated benchmark suite (`criterion`, `cargo bench`)
/// that runs in a controlled environment, not an `assert!` on
/// `Instant::now()` deltas inside the `cargo test` flow.
#[proc_macro_attribute]
pub fn bench_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_body = &input_fn.block;

    let timing_code = quote! {
        {
            let start_time = std::time::Instant::now();
            let result = #fn_body;
            let end_time = std::time::Instant::now();
            let time =  (end_time - start_time).as_micros();
            println!("{} took {} μs", stringify!(#fn_name), (end_time - start_time).as_micros());
            // 5000 ms — see bench_test docstring for rationale.
            assert!(time < 5_000_000, "Bench mark test failed");
            result
        }
    };

    input_fn.block = Box::new(syn::parse2(timing_code).unwrap());

    let output = quote! {
        #input_fn
    };

    output.into()
}
