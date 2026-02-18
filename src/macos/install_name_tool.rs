//! A minimal implementation of `install_name_tool` for cross-compilation.
//!
//! Supports modifying Mach-O load commands:
//! - `-id name`: Change LC_ID_DYLIB
//! - `-change old new`: Change LC_LOAD_DYLIB / LC_LOAD_WEAK_DYLIB / etc.
//! - `-add_rpath new`: Add LC_RPATH
//! - `-delete_rpath old`: Delete LC_RPATH
//! - `-rpath old new`: Change LC_RPATH
//!
//! Based on the approach from [arwen-macho](https://github.com/nichmor/arwen).
//!
//! TODO: Replace this custom implementation with the `arwen-macho` crate
//! once it's published to crates.io.

use std::ffi::{CStr, OsString};
use std::path::Path;

use anyhow::{Context, Result, bail};
use goblin::container;
use goblin::mach::fat;
use goblin::mach::header::{Header, SIZEOF_HEADER_32, SIZEOF_HEADER_64};
use goblin::mach::load_command::{
    CommandVariant, DylibCommand, LC_RPATH, LoadCommand, RpathCommand, SIZEOF_RPATH_COMMAND,
};
use goblin::mach::{MachO, MultiArch, parse_magic_and_ctx, peek};
use scroll::Pwrite;

/// Parsed command-line arguments for install_name_tool
#[derive(Debug, Default)]
struct Args {
    id: Option<String>,
    changes: Vec<(String, String)>,
    rpaths: Vec<(String, String)>,
    add_rpaths: Vec<String>,
    delete_rpaths: Vec<String>,
    input: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args> {
    let mut parsed = Args::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-id" => {
                if i + 1 >= args.len() {
                    bail!("-id requires an argument");
                }
                parsed.id = Some(args[i + 1].clone());
                i += 2;
            }
            "-change" => {
                if i + 2 >= args.len() {
                    bail!("-change requires two arguments");
                }
                parsed
                    .changes
                    .push((args[i + 1].clone(), args[i + 2].clone()));
                i += 3;
            }
            "-rpath" => {
                if i + 2 >= args.len() {
                    bail!("-rpath requires two arguments");
                }
                parsed
                    .rpaths
                    .push((args[i + 1].clone(), args[i + 2].clone()));
                i += 3;
            }
            "-add_rpath" => {
                if i + 1 >= args.len() {
                    bail!("-add_rpath requires an argument");
                }
                parsed.add_rpaths.push(args[i + 1].clone());
                i += 2;
            }
            "-delete_rpath" => {
                if i + 1 >= args.len() {
                    bail!("-delete_rpath requires an argument");
                }
                parsed.delete_rpaths.push(args[i + 1].clone());
                i += 2;
            }
            arg if arg.starts_with('-') => {
                bail!("unknown option: {arg}");
            }
            _ => {
                if parsed.input.is_some() {
                    bail!("multiple input files not supported");
                }
                parsed.input = Some(args[i].clone());
                i += 1;
            }
        }
    }
    if parsed.input.is_none() {
        bail!("no input file specified");
    }
    Ok(parsed)
}

// -- Header helpers --

fn header_size(ctx: container::Ctx) -> usize {
    if ctx.container.is_big() {
        SIZEOF_HEADER_64
    } else {
        SIZEOF_HEADER_32
    }
}

/// Align size to pointer width (4 for 32-bit, 8 for 64-bit)
fn align_to_ctx(size: usize, ctx: container::Ctx) -> usize {
    if ctx.container.is_big() {
        size.next_multiple_of(8)
    } else {
        size.next_multiple_of(4)
    }
}

// -- Load command manipulation --

/// Remove a load command from the buffer and update the header.
fn remove_load_command(
    buffer: &mut Vec<u8>,
    header: &mut Header,
    ctx: container::Ctx,
    cmd_offset: usize,
    cmdsize: usize,
) -> Result<()> {
    buffer.drain(cmd_offset..cmd_offset + cmdsize);

    header.ncmds -= 1;
    header.sizeofcmds -= cmdsize as u32;

    // Insert zero padding after remaining load commands to keep file size stable
    let padding_offset = header_size(ctx) + header.sizeofcmds as usize;
    let zeroes = vec![0u8; cmdsize];
    let tail = buffer.split_off(padding_offset);
    buffer.extend(&zeroes);
    buffer.extend(tail);

    buffer.pwrite_with(*header, 0, ctx)?;
    Ok(())
}

/// Insert a new load command at the given offset and update the header.
fn insert_load_command(
    buffer: &mut Vec<u8>,
    header: &mut Header,
    ctx: container::Ctx,
    offset: usize,
    cmd_data: &[u8],
) -> Result<()> {
    let new_cmd_size = cmd_data.len() as u32;

    header.ncmds += 1;
    header.sizeofcmds += new_cmd_size;

    // Insert the new command bytes
    let tail = buffer.split_off(offset);
    buffer.extend_from_slice(cmd_data);
    buffer.extend(tail);

    // Drain surplus padding to keep file size stable
    let drain_start = header_size(ctx) + header.sizeofcmds as usize;
    let drain_end = drain_start + new_cmd_size as usize;
    if drain_end <= buffer.len() {
        buffer.drain(drain_start..drain_end);
    }

    buffer.pwrite_with(*header, 0, ctx)?;
    Ok(())
}

/// Build a serialized LC_RPATH command
fn build_rpath_command(path: &str, ctx: container::Ctx) -> Result<(RpathCommand, Vec<u8>)> {
    let c_str = format!("{path}\0");
    let c_str = CStr::from_bytes_with_nul(c_str.as_bytes())?;
    let str_size = (c_str.count_bytes() + 1).next_multiple_of(4);
    let cmdsize = align_to_ctx(SIZEOF_RPATH_COMMAND + str_size, ctx);

    let rpath_cmd = RpathCommand {
        cmd: LC_RPATH,
        cmdsize: cmdsize as u32,
        path: SIZEOF_RPATH_COMMAND as u32,
    };

    let mut buf = vec![0u8; cmdsize];
    buf.pwrite(rpath_cmd, 0)?;
    buf.pwrite(c_str, SIZEOF_RPATH_COMMAND)?;
    Ok((rpath_cmd, buf))
}

/// Build a serialized DylibCommand (for LC_ID_DYLIB, LC_LOAD_DYLIB, etc.)
fn build_dylib_command(
    name: &str,
    old_cmd: &DylibCommand,
    ctx: container::Ctx,
) -> Result<(DylibCommand, Vec<u8>)> {
    let c_str = format!("{name}\0");
    let c_str = CStr::from_bytes_with_nul(c_str.as_bytes())?;
    let str_size = (c_str.count_bytes() + 1).next_multiple_of(4);
    // DylibCommand header: cmd(4) + cmdsize(4) + name_offset(4) + timestamp(4) + current_version(4) + compat_version(4) = 24
    let dylib_header_size: usize = 24;
    let cmdsize = align_to_ctx(dylib_header_size + str_size, ctx);

    let new_cmd = DylibCommand {
        cmd: old_cmd.cmd,
        cmdsize: cmdsize as u32,
        dylib: goblin::mach::load_command::Dylib {
            name: dylib_header_size as u32,
            timestamp: old_cmd.dylib.timestamp,
            current_version: old_cmd.dylib.current_version,
            compatibility_version: old_cmd.dylib.compatibility_version,
        },
    };

    let mut buf = vec![0u8; cmdsize];
    buf.pwrite(new_cmd, 0)?;
    buf.pwrite(c_str, dylib_header_size)?;
    Ok((new_cmd, buf))
}

// -- Command finders --

/// Read the string name from a dylib load command in the raw data
fn read_dylib_name<'a>(data: &'a [u8], lc: &LoadCommand, dylib_cmd: &DylibCommand) -> &'a str {
    let name_offset = lc.offset + dylib_cmd.dylib.name as usize;
    let cmd_end = lc.offset + dylib_cmd.cmdsize as usize;
    let name_end = data[name_offset..cmd_end]
        .iter()
        .position(|&b| b == 0)
        .map(|p| name_offset + p)
        .unwrap_or(cmd_end);
    std::str::from_utf8(&data[name_offset..name_end]).unwrap_or("")
}

/// Read the string path from an rpath load command in the raw data
fn read_rpath_path<'a>(data: &'a [u8], lc: &LoadCommand, rpath_cmd: &RpathCommand) -> &'a str {
    let path_offset = lc.offset + rpath_cmd.path as usize;
    let cmd_end = lc.offset + rpath_cmd.cmdsize as usize;
    let path_end = data[path_offset..cmd_end]
        .iter()
        .position(|&b| b == 0)
        .map(|p| path_offset + p)
        .unwrap_or(cmd_end);
    std::str::from_utf8(&data[path_offset..path_end]).unwrap_or("")
}

// -- Single Mach-O processing --

/// Process a single Mach-O binary. The buffer must start at the Mach-O header (offset 0).
fn process_single_macho(data: &mut Vec<u8>, args: &Args) -> Result<()> {
    let macho = MachO::parse(data, 0).context("failed to parse Mach-O")?;
    let (_, maybe_ctx) = parse_magic_and_ctx(data, 0)?;
    let ctx = maybe_ctx.context("could not determine endianness")?;
    let mut header = macho.header;

    // -id: change LC_ID_DYLIB
    if let Some(ref new_id) = args.id {
        let mut found = false;
        for lc in &macho.load_commands {
            if let CommandVariant::IdDylib(ref dylib_cmd) = lc.command {
                let cmdsize = lc.command.cmdsize();
                let (_, new_cmd_buf) = build_dylib_command(new_id, dylib_cmd, ctx)?;
                remove_load_command(data, &mut header, ctx, lc.offset, cmdsize)?;
                insert_load_command(data, &mut header, ctx, lc.offset, &new_cmd_buf)?;
                found = true;
                break;
            }
        }
        if !found {
            bail!("no LC_ID_DYLIB found in binary");
        }
    }

    // After modifying the binary, we need to re-parse to get updated offsets.
    // For -change, -rpath, -delete_rpath, -add_rpath we re-parse each time.

    // -change: change dylib load names
    for (old_name, new_name) in &args.changes {
        let macho = MachO::parse(data, 0).context("failed to re-parse Mach-O")?;
        let (_, maybe_ctx) = parse_magic_and_ctx(data, 0)?;
        let ctx = maybe_ctx.context("could not determine endianness")?;
        let mut header = macho.header;

        let mut found = false;
        for lc in &macho.load_commands {
            let dylib_cmd = match &lc.command {
                CommandVariant::LoadDylib(cmd)
                | CommandVariant::LoadWeakDylib(cmd)
                | CommandVariant::ReexportDylib(cmd)
                | CommandVariant::LazyLoadDylib(cmd)
                | CommandVariant::LoadUpwardDylib(cmd) => cmd,
                _ => continue,
            };
            let name = read_dylib_name(data, lc, dylib_cmd);
            if name == old_name.as_str() {
                let cmdsize = lc.command.cmdsize();
                let (_, new_cmd_buf) = build_dylib_command(new_name, dylib_cmd, ctx)?;
                remove_load_command(data, &mut header, ctx, lc.offset, cmdsize)?;
                insert_load_command(data, &mut header, ctx, lc.offset, &new_cmd_buf)?;
                found = true;
                break;
            }
        }
        if !found {
            bail!("no LC_LOAD_DYLIB with name '{old_name}' found");
        }
    }

    // -rpath: change rpath
    for (old_rpath, new_rpath) in &args.rpaths {
        let macho = MachO::parse(data, 0).context("failed to re-parse Mach-O")?;
        let (_, maybe_ctx) = parse_magic_and_ctx(data, 0)?;
        let ctx = maybe_ctx.context("could not determine endianness")?;
        let mut header = macho.header;

        let mut found = false;
        for lc in &macho.load_commands {
            if let CommandVariant::Rpath(ref rpath_cmd) = lc.command {
                let path = read_rpath_path(data, lc, rpath_cmd);
                if path == old_rpath.as_str() {
                    let cmdsize = lc.command.cmdsize();
                    let (_, new_cmd_buf) = build_rpath_command(new_rpath, ctx)?;
                    remove_load_command(data, &mut header, ctx, lc.offset, cmdsize)?;
                    insert_load_command(data, &mut header, ctx, lc.offset, &new_cmd_buf)?;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            bail!("no LC_RPATH with path '{old_rpath}' found");
        }
    }

    // -delete_rpath
    for del_rpath in &args.delete_rpaths {
        let macho = MachO::parse(data, 0).context("failed to re-parse Mach-O")?;
        let (_, maybe_ctx) = parse_magic_and_ctx(data, 0)?;
        let ctx = maybe_ctx.context("could not determine endianness")?;
        let mut header = macho.header;

        let mut found = false;
        for lc in &macho.load_commands {
            if let CommandVariant::Rpath(ref rpath_cmd) = lc.command {
                let path = read_rpath_path(data, lc, rpath_cmd);
                if path == del_rpath.as_str() {
                    let cmdsize = lc.command.cmdsize();
                    remove_load_command(data, &mut header, ctx, lc.offset, cmdsize)?;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            bail!("no LC_RPATH with path '{del_rpath}' found");
        }
    }

    // -add_rpath
    for new_rpath in &args.add_rpaths {
        let macho = MachO::parse(data, 0).context("failed to re-parse Mach-O")?;
        let (_, maybe_ctx) = parse_magic_and_ctx(data, 0)?;
        let ctx = maybe_ctx.context("could not determine endianness")?;
        let mut header = macho.header;

        let insert_offset = header_size(ctx) + header.sizeofcmds as usize;
        let (_, new_cmd_buf) = build_rpath_command(new_rpath, ctx)?;
        insert_load_command(data, &mut header, ctx, insert_offset, &new_cmd_buf)?;
    }

    Ok(())
}

// -- Top-level file processing --

fn process_file(path: &Path, args: &Args) -> Result<()> {
    let mut data =
        fs_err::read(path).with_context(|| format!("failed to read '{}'", path.display()))?;

    let magic = peek(&data, 0)?;

    match magic {
        fat::FAT_MAGIC => {
            let multi = MultiArch::new(&data)?;
            let arches: Vec<_> = multi.iter_arches().collect::<std::result::Result<_, _>>()?;

            // Process each arch slice independently, then splice it back.
            // Process from last to first so that offset changes don't affect earlier slices.
            for arch in arches.iter().rev() {
                let offset = arch.offset as usize;
                let size = arch.size as usize;
                let mut slice = data[offset..offset + size].to_vec();
                process_single_macho(&mut slice, args)?;
                data.splice(offset..offset + size, slice);
            }
        }
        _ => {
            // Single Mach-O (or will fail inside process_single_macho)
            process_single_macho(&mut data, args)?;
        }
    }

    fs_err::write(path, &data).with_context(|| format!("failed to write '{}'", path.display()))?;

    Ok(())
}

/// Execute install_name_tool with the given arguments
pub fn execute(args: impl IntoIterator<Item = impl Into<OsString>>) -> Result<()> {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.into().to_string_lossy().into_owned())
        .collect();
    let parsed = parse_args(&args)?;
    let input = parsed.input.as_ref().unwrap();
    process_file(Path::new(input), &parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Copy a fixture to a temp file for modification
    fn copy_fixture(name: &str) -> tempfile::NamedTempFile {
        let src = fixtures_dir().join(name);
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::copy(&mut fs_err::File::open(src).unwrap(), &mut tmp).unwrap();
        tmp
    }

    /// Read the LC_ID_DYLIB name from a single Mach-O slice
    fn read_id(data: &[u8]) -> Option<String> {
        let macho = MachO::parse(data, 0).unwrap();
        macho.name.map(|s| s.to_string())
    }

    /// Read all rpaths from a single Mach-O slice
    fn read_rpaths(data: &[u8]) -> Vec<String> {
        let macho = MachO::parse(data, 0).unwrap();
        macho.rpaths.iter().map(|s| s.to_string()).collect()
    }

    /// For fat binaries, get the slices as (offset, size) pairs
    fn fat_slices(data: &[u8]) -> Vec<(usize, usize)> {
        let multi = MultiArch::new(data).unwrap();
        multi
            .iter_arches()
            .map(|a| {
                let a = a.unwrap();
                (a.offset as usize, a.size as usize)
            })
            .collect()
    }

    // -- Tests for single-arch (aarch64) --

    #[test]
    fn test_change_id_aarch64() {
        let tmp = copy_fixture("test_aarch64.dylib");
        execute(["-id", "/new/lib/test.dylib", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        assert_eq!(read_id(&data).as_deref(), Some("/new/lib/test.dylib"));
    }

    #[test]
    fn test_change_rpath_aarch64() {
        let tmp = copy_fixture("test_aarch64.dylib");
        execute([
            "-rpath",
            "/old/rpath",
            "/new/rpath",
            tmp.path().to_str().unwrap(),
        ])
        .unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        let rpaths = read_rpaths(&data);
        assert_eq!(rpaths, vec!["/new/rpath"]);
    }

    #[test]
    fn test_delete_rpath_aarch64() {
        let tmp = copy_fixture("test_aarch64.dylib");
        execute(["-delete_rpath", "/old/rpath", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        let rpaths = read_rpaths(&data);
        assert!(rpaths.is_empty());
    }

    #[test]
    fn test_add_rpath_aarch64() {
        let tmp = copy_fixture("test_aarch64.dylib");
        execute(["-add_rpath", "/added/rpath", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        let rpaths = read_rpaths(&data);
        assert!(rpaths.contains(&"/old/rpath".to_string()));
        assert!(rpaths.contains(&"/added/rpath".to_string()));
    }

    // -- Tests for single-arch (x86_64) --

    #[test]
    fn test_change_id_x86_64() {
        let tmp = copy_fixture("test_x86_64.dylib");
        execute(["-id", "/new/lib/test.dylib", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        assert_eq!(read_id(&data).as_deref(), Some("/new/lib/test.dylib"));
    }

    #[test]
    fn test_change_rpath_x86_64() {
        let tmp = copy_fixture("test_x86_64.dylib");
        execute([
            "-rpath",
            "/old/rpath",
            "/new/rpath",
            tmp.path().to_str().unwrap(),
        ])
        .unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        let rpaths = read_rpaths(&data);
        assert_eq!(rpaths, vec!["/new/rpath"]);
    }

    // -- Tests for fat (universal2) binary --

    #[test]
    fn test_change_id_universal2() {
        let tmp = copy_fixture("test_universal2.dylib");
        execute(["-id", "/new/lib/test.dylib", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        for (offset, size) in fat_slices(&data) {
            let slice = &data[offset..offset + size];
            assert_eq!(read_id(slice).as_deref(), Some("/new/lib/test.dylib"));
        }
    }

    #[test]
    fn test_change_rpath_universal2() {
        let tmp = copy_fixture("test_universal2.dylib");
        execute([
            "-rpath",
            "/old/rpath",
            "/new/rpath",
            tmp.path().to_str().unwrap(),
        ])
        .unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        for (offset, size) in fat_slices(&data) {
            let slice = &data[offset..offset + size];
            assert_eq!(read_rpaths(slice), vec!["/new/rpath"]);
        }
    }

    #[test]
    fn test_delete_rpath_universal2() {
        let tmp = copy_fixture("test_universal2.dylib");
        execute(["-delete_rpath", "/old/rpath", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        for (offset, size) in fat_slices(&data) {
            let slice = &data[offset..offset + size];
            assert!(read_rpaths(slice).is_empty());
        }
    }

    #[test]
    fn test_add_rpath_universal2() {
        let tmp = copy_fixture("test_universal2.dylib");
        execute(["-add_rpath", "/added/rpath", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        for (offset, size) in fat_slices(&data) {
            let slice = &data[offset..offset + size];
            let rpaths = read_rpaths(slice);
            assert!(rpaths.contains(&"/old/rpath".to_string()));
            assert!(rpaths.contains(&"/added/rpath".to_string()));
        }
    }

    // -- Combined operations --

    #[test]
    fn test_multiple_operations_aarch64() {
        let tmp = copy_fixture("test_aarch64.dylib");
        // Change id and rpath in separate calls (like real usage)
        execute(["-id", "/new/id.dylib", tmp.path().to_str().unwrap()]).unwrap();
        execute([
            "-rpath",
            "/old/rpath",
            "/replaced/rpath",
            tmp.path().to_str().unwrap(),
        ])
        .unwrap();
        execute(["-add_rpath", "/extra/rpath", tmp.path().to_str().unwrap()]).unwrap();

        let data = fs_err::read(tmp.path()).unwrap();
        assert_eq!(read_id(&data).as_deref(), Some("/new/id.dylib"));
        let rpaths = read_rpaths(&data);
        assert!(rpaths.contains(&"/replaced/rpath".to_string()));
        assert!(rpaths.contains(&"/extra/rpath".to_string()));
        assert!(!rpaths.contains(&"/old/rpath".to_string()));
    }

    // -- Error cases --

    #[test]
    fn test_delete_nonexistent_rpath_fails() {
        let tmp = copy_fixture("test_aarch64.dylib");
        let result = execute([
            "-delete_rpath",
            "/nonexistent",
            tmp.path().to_str().unwrap(),
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_change_nonexistent_rpath_fails() {
        let tmp = copy_fixture("test_aarch64.dylib");
        let result = execute([
            "-rpath",
            "/nonexistent",
            "/new",
            tmp.path().to_str().unwrap(),
        ]);
        assert!(result.is_err());
    }
}
