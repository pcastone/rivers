pub fn cmd_status(_args: &[String]) -> Result<(), String> {
    let pid = match super::stop::read_pid_file() {
        Ok(pid) => pid,
        Err(_) => {
            println!("rivers: riversd is not running (no PID file)");
            return Ok(());
        }
    };
    if !super::stop::is_process_alive(pid) {
        println!("rivers: riversd is not running (stale PID file, pid {pid})");
        super::stop::cleanup_pid_file();
        return Ok(());
    }
    println!("rivers: riversd is running (pid {pid})");
    if let Some(config_path) = super::doctor::discover_config() {
        println!("  config: {}", config_path.display());
        if let Ok(cfg) = rivers_runtime::loader::load_server_config(&config_path) {
            println!("  port:   {}", cfg.base.port);
            if let Some(ref bundle) = cfg.bundle_path {
                println!("  bundle: {bundle}");
            }
        }
    }
    Ok(())
}
