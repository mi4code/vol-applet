use std::{sync::Arc, env};

use once_cell::sync::Lazy;

use crate::exception::Exception;

pub static OPTIONS: Lazy<Arc<Options>> = Lazy::new(|| {
    let args: Vec<String> = env::args().skip(1).collect();
    let options = match Options::from_args(args) {
        Ok(options) => options,
        Err(e) => {
            e.log_and_exit();
            unreachable!();
        }
    };

    Arc::new(options)
});

pub struct Options {
    pub show_inputs: bool,
    pub show_streams: bool,
    pub show_icons: bool,
    pub dont_group: bool,
    // Optional explicit placement: when provided the popout will be shown at
    // these coordinates immediately (skipping tray-icon geometry). Works on
    // both X11 and Wayland; on Wayland the code will use gtk-layer-shell.
    pub place: Option<(i32, i32)>,
}

impl Options {
    pub fn from_args(args: Vec<String>) -> Result<Options, Exception> {
        let mut options = Options::default();

        let args = split_small_flags(args);

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-i" | "--show-inputs" => options.show_inputs = true,
                "-s" | "--show-streams" => options.show_streams = true,
                "-d" | "--dont-group" => options.dont_group = true,
                "-c" | "--show-icons" => options.show_icons = true,
                "-h" | "--help" => {
                    help();
                    unreachable!();
                }
                "--place" => {
                    // Expect two following args: X Y
                    if i + 2 >= args.len() {
                        return Err(Exception::Misc("--place requires two arguments: X Y".to_string()));
                    }
                    let x = args[i + 1].parse::<i32>().map_err(|_| Exception::Misc("Invalid X coordinate for --place".to_string()))?;
                    let y = args[i + 2].parse::<i32>().map_err(|_| Exception::Misc("Invalid Y coordinate for --place".to_string()))?;
                    options.place = Some((x, y));
                    i += 2; // skip the coordinates
                }
                _ => {
                    return Err(Exception::Misc(format!("Unknown option: {}", arg)));
                }
            }
            i += 1;
        }

        Ok(options)
    }
}

fn split_small_flags(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .map(|arg| {
            if is_small_flags(&arg) {
                arg.chars().skip(1).map(|c| format!("-{}", c)).collect()
            } else {
                vec![arg]
            }
        })
        .flatten()
        .collect()
}

fn is_small_flags(arg: &String) -> bool {
    if arg.len() < 2 {
        return false;
    }

    if !arg.starts_with("-") {
        return false;
    }

    arg.chars().skip(1).all(|c| c.is_alphabetic())
}

fn help() {
    println!("Usage: volapplet [options]");
    println!();
    println!("Options:");
    println!("  -i, --show-inputs       Show input devices.");
    println!("  -s, --show-streams      Show streams.");
    println!("  -c, --show-icons        Show icons.");
    println!("  -d, --dont-group        Don't group streams and inputs into expandable tabs.");
    println!("      --place X Y         Immediately show the popout at coordinates X Y (pixels). On Wayland this uses gtk-layer-shell.");
    println!("  -h, --help              Show this help message and exit.");

    std::process::exit(0);
}

impl Default for Options {
    fn default() -> Self {
        Options {
            show_inputs: false,
            show_streams: false,
            show_icons: false,
            dont_group: false,
            place: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let args = vec!["-i".to_string(), "-sc".to_string(), "--dont-group".to_string()];
        let options = Options::from_args(args).unwrap();

        assert!(options.show_inputs);
        assert!(options.show_streams);
        assert!(options.show_icons);
        assert!(options.dont_group);

        let args = vec!["-ds".to_string()];
        let options = Options::from_args(args).unwrap();

        assert!(!options.show_inputs);
        assert!(options.show_streams);
        assert!(!options.show_icons);
        assert!(options.dont_group);

        let args = vec!["a".to_string()];
        assert!(Options::from_args(args).is_err());

        // test --place parsing
        let args = vec!["--place".to_string(), "100".to_string(), "200".to_string()];
        let options = Options::from_args(args).unwrap();
        assert_eq!(options.place, Some((100, 200)));
    }
}
