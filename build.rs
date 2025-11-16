use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Only attempt to install man pages if they exist (i.e., if xtask has been run)
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let man_dir = manifest_dir.join("target").join("man");

    if !man_dir.exists() {
        // Man pages haven't been generated yet, that's okay
        return;
    }

    // During cargo install, we can detect the installation and try to install man pages
    if let Ok(install_root) = env::var("CARGO_INSTALL_ROOT") {
        // Determine the appropriate man page directory
        let install_path = PathBuf::from(&install_root);

        // Try common man page locations
        let mut possible_man_dirs = vec![
            install_path.join("share/man/man1"), // For /usr/local, ~/.cargo, etc.
        ];

        // Add user home directory option if available
        if let Ok(home) = env::var("HOME") {
            possible_man_dirs.push(PathBuf::from(home).join(".local/share/man/man1"));
        }

        // Try /usr/local/share/man/man1 as a fallback
        possible_man_dirs.push(PathBuf::from("/usr/local/share/man/man1"));

        for target_dir in &possible_man_dirs {
            // Try to create the directory and copy files
            if let Ok(()) = fs::create_dir_all(target_dir) {
                let man_files = [
                    "artsum.1",
                    "artsum-generate.1",
                    "artsum-verify.1",
                    "artsum-refresh.1",
                ];

                let mut any_success = false;
                for file in &man_files {
                    let src = man_dir.join(file);
                    let dst = target_dir.join(file);

                    if src.exists() {
                        if let Ok(()) = fs::copy(&src, &dst).map(|_| ()) {
                            println!("cargo:warning=Installed man page: {:?}", dst);
                            any_success = true;
                        }
                    }
                }

                if any_success {
                    println!("cargo:warning=Man pages installed to {:?}", target_dir);
                    break;
                }
            }
        }
    }
}
