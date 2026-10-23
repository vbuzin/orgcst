#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    orgcst_fuzz::fuzz_apply(data);
});
