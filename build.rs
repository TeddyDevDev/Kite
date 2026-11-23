fn main() {
    slint_build::compile("ui/app.slint").expect("no se pudo compilar la interfaz");

    // Icono y datos del .exe en Windows (si falla, se compila igual pero sin icono)
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/kite.ico");
        res.set("FileDescription", "Kite");
        res.set("ProductName", "Kite");
        if let Err(e) = res.compile() {
            println!("cargo:warning=No se pudo incrustar el icono: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/kite.ico");
}
