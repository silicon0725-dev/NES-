use nes_scene::*;
fn ops(src: &str) -> Vec<Op> {
    compile_script(src).map(|s| s.ops).unwrap_or_else(|e| panic!("{e}"))
}
#[test]
fn pr() {
    eprintln!("and: {:?}", ops("on \"x\" { if a == 1 && b == 2 { } }"));
    eprintln!("ne: {:?}", ops("on \"x\" { if 3 != 3 { } }"));
    eprintln!("le: {:?}", ops("on \"x\" { if 2 <= 2 { } }"));
    eprintln!("ge: {:?}", ops("on \"x\" { if 3 >= 4 { } }"));
    eprintln!("gt: {:?}", ops("on \"x\" { if a > b { } }"));
}
