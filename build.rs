fn main() {
    println!("cargo:rerun-if-changed=assets/square-masher.ico");
    println!("cargo:rerun-if-changed=assets/square-masher.png");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/square-masher.ico")
            .compile()
            .expect("failed to embed SquareMasher Windows icon");
    }

    slint_build::compile("ui/app.slint").expect("failed to compile SquareMasher UI");
}
