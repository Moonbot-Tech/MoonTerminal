//! `symbolize <moonterminal.pdb> <rva>...` — the developer's half of a Windows crash report.
//!
//! `panic.log` names the faulting instruction and the stack as `moonterminal+0xRVA`: offsets
//! into the executable, the only form a stripped build can produce. This turns them into
//! function, file and line against a PDB. The PDB is not kept anywhere: the release profile
//! links with line tables and throws it away, and a build of the same tag with the same profile
//! reproduces the code layout byte for byte (measured on two clean builds: `.text` identical),
//! so the PDB from a local rebuild describes the shipped executable. Recipe:
//!
//! ```text
//! git checkout v0.43.7
//! $env:CARGO_PROFILE_RELEASE_STRIP = "false"; $env:CARGO_PROFILE_RELEASE_DEBUG = "line-tables-only"
//! cargo build -p moon-ui-gpui --bin moonterminal --release --locked --target x86_64-pc-windows-msvc
//! cargo run --manifest-path tools\symbolize\Cargo.toml -- target\x86_64-pc-windows-msvc\release\moonterminal.pdb 0x61ED99
//! ```
//!
//! An offset is `0x…` on its own or a whole pasted `panic.log` line, whose `moonterminal+0x…` is
//! what gets read; anything else is reported as skipped. The PDB must come from the SAME tag and
//! the same overrides — the tool cannot tell a wrong PDB from a right one, only the build line in
//! the report can.

use std::fmt::Write as _;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(pdb_path) = args.next() else {
        eprintln!(
            "usage: symbolize <moonterminal.pdb> <rva>...   (rva as 0x1BF52D or a panic.log line)"
        );
        std::process::exit(2);
    };
    let mut rvas = Vec::new();
    for arg in args {
        match parse_rva(&arg) {
            Some(rva) => rvas.push(rva),
            None => {
                eprintln!(
                    "skipped (not `moonterminal+0x…`, nor a 0x-prefixed offset that fits u32): {arg}"
                )
            }
        }
    }
    if rvas.is_empty() {
        eprintln!("no offsets given");
        std::process::exit(2);
    }
    match run(&pdb_path, &rvas) {
        Ok(text) => print!("{text}"),
        Err(e) => {
            eprintln!("symbolize: {e}");
            std::process::exit(1);
        }
    }
}

/// The offset in an argument: the `moonterminal+0x…` inside a report line (text after the
/// digits is ignored), or a `0x`-prefixed number on its own. A bare word is refused — `at`,
/// `bad` and `face` are hex too — and so is another image's frame (`ntdll+0x…`): either read
/// as an offset would symbolize to something that looks right and is not.
fn parse_rva(arg: &str) -> Option<u32> {
    const OURS: &str = "moonterminal+0x";
    let trimmed = arg.trim();
    let (hex, in_line) = match trimmed.find(OURS) {
        // A frame of another image (`ntdll+0x…`) is not an offset into our PDB, whatever it
        // says after the plus.
        Some(p) => (&trimmed[p + OURS.len()..], true),
        None => (
            trimmed
                .strip_prefix("0x")
                .or_else(|| trimmed.strip_prefix("0X"))?,
            false,
        ),
    };
    let digits: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if digits.is_empty() || (!in_line && digits.len() != hex.len()) {
        return None;
    }
    u32::from_str_radix(&digits, 16).ok()
}

fn run(pdb_path: &str, rvas: &[u32]) -> Result<String, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(pdb_path)?;
    let pdb = pdb_addr2line::pdb::PDB::open(file)?;
    let data = pdb_addr2line::ContextPdbData::try_from_pdb(pdb)?;
    let context = data.make_context()?;
    let mut out = String::new();
    for rva in rvas {
        match context.find_frames(*rva)? {
            Some(procedure) => {
                for (i, frame) in procedure.frames.iter().enumerate() {
                    let what = if i == 0 { "" } else { "  (inlined into) " };
                    let _ = writeln!(
                        out,
                        "moonterminal+0x{rva:X}: {what}{}  {}:{}",
                        frame.function.as_deref().unwrap_or("?"),
                        frame.file.as_deref().unwrap_or("?"),
                        frame
                            .line
                            .map_or_else(|| "?".to_string(), |l| l.to_string())
                    );
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "moonterminal+0x{rva:X}: not inside any function the PDB knows"
                );
            }
        }
    }
    Ok(out)
}
