//! U-Boot environment tool for the X19 control board.
//!
//!   omo-bootenv get [name…]
//!   omo-bootenv set name=value…     (empty value deletes the variable)
//!   omo-bootenv boot-braiins         firmware=1, upgrade_stage removed
//!   omo-bootenv keep-openmineros     firmware=2, upgrade_stage removed (no auto-revert)

#[cfg(unix)]
fn main() {
    use openmineros_board_xil::uboot_env::hw;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res: Result<(), String> = (|| match args.first().map(String::as_str) {
        Some("get") => {
            let (copies, a) = hw::read()?;
            let env = &copies[a].as_ref().ok_or("no env")?.0;
            if args.len() == 1 {
                for (k, v) in &env.vars {
                    println!("{k}={v}");
                }
            } else {
                for n in &args[1..] {
                    println!("{n}={}", env.get(n).unwrap_or(""));
                }
            }
            Ok(())
        }
        Some("set") => {
            let pairs: Vec<(String, String)> = args[1..]
                .iter()
                .map(|a| {
                    a.split_once('=')
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .ok_or(format!("expected name=value, got {a}"))
                })
                .collect::<Result<_, _>>()?;
            let refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            hw::update(&refs)?;
            println!("ok");
            Ok(())
        }
        Some("boot-braiins") => hw::update(&[("firmware", "1"), ("upgrade_stage", "")])
            .map(|_| println!("next boot: Braiins OS (slot 1)")),
        Some("keep-openmineros") => hw::update(&[("firmware", "2"), ("upgrade_stage", "")])
            .map(|_| println!("OpenMinerOS (slot 2) is now permanent")),
        _ => Err(
            "usage: omo-bootenv get [name…] | set name=value… | boot-braiins | keep-openmineros"
                .into(),
        ),
    })();
    if let Err(e) = res {
        eprintln!("omo-bootenv: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {}
