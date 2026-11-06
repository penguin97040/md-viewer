//! Size baseline: a binary that links this crate but calls nothing from it.
fn main() {
    println!("{}", std::env::args().count());
}
