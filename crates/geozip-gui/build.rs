fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut resources = winresource::WindowsResource::new();
        resources.set_icon("../../assets/icon.ico");
        resources.compile().expect("the Windows icon resource compiles");
    }
}
