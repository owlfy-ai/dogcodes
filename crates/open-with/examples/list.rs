fn main() {
    let path = std::env::args_os().nth(1).expect("file path");
    let apps = open_with::applications(std::path::Path::new(&path)).expect("query applications");
    if std::env::args_os()
        .nth(2)
        .is_some_and(|arg| arg == "--json")
    {
        println!(
            "{}",
            serde_json::to_string(&apps).expect("serialize applications")
        );
        return;
    }
    for app in apps {
        println!(
            "{}{} | {} | icon: {}",
            app.name,
            if app.is_default { " (default)" } else { "" },
            app.id,
            app.icon.is_some()
        );
    }
}
