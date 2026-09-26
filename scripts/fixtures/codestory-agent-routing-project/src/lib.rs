pub mod one;
pub mod two;

pub fn finish() {}

pub fn start() {
    finish();
}

pub fn detour() {
    finish();
}

pub fn refuted_start() {
    detour();
}

pub fn dynamic_start(target: fn()) {
    target();
}
