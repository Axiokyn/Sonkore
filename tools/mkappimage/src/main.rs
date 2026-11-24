//! Builds an AppImage without appimagetool: the type 2 runtime (taken from an
//! existing AppImage or a runtime file) followed by a zstd squashfs of the
//! AppDir, 128 KiB blocks, root-owned files with their permissions.
//!   mkappimage <AppDir> <runtime or AppImage> <output.AppImage>

use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use backhand::compression::Compressor;
use backhand::{FilesystemCompressor, FilesystemWriter, NodeHeader};

/// The runtime: a whole file, or the ELF part of a type 2 AppImage.
fn runtime(path: &Path) -> Vec<u8> {
    let data = fs::read(path).expect("runtime");
    assert_eq!(&data[..4], b"\x7fELF", "not an ELF file");
    let shoff = u64::from_le_bytes(data[0x28..0x30].try_into().unwrap()) as usize;
    let shentsize = u16::from_le_bytes(data[0x3a..0x3c].try_into().unwrap()) as usize;
    let shnum = u16::from_le_bytes(data[0x3c..0x3e].try_into().unwrap()) as usize;
    let end = shoff + shentsize * shnum;
    data[..end.min(data.len())].to_vec()
}

fn walk(root: &Path, dir: &Path, fs: &mut FilesystemWriter, mtime: u32) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).expect("read dir").flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let rel = Path::new("/").join(path.strip_prefix(root).unwrap());
        let meta = fs::symlink_metadata(&path).expect("metadata");
        let header = NodeHeader::new((meta.mode() & 0o7777) as u16, 0, 0, mtime);
        if meta.file_type().is_symlink() {
            fs.push_symlink(fs::read_link(&path).unwrap(), &rel, header).expect("symlink");
        } else if meta.is_dir() {
            fs.push_dir(&rel, header).expect("dir");
            walk(root, &path, fs, mtime);
        } else {
            fs.push_file_from_path(path.clone(), &rel, header).expect("file");
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("mkappimage <AppDir> <runtime or AppImage> <output.AppImage>");
        std::process::exit(2);
    }
    let (appdir, runtime_path, out) = (Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3]));
    let mtime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
    let mut squash = FilesystemWriter::default();
    squash.set_time(mtime);
    squash.set_block_size(128 * 1024);
    squash.set_compressor(FilesystemCompressor::new(Compressor::Zstd, None).expect("zstd"));
    squash.set_root_mode(0o755);
    squash.set_root_uid(0);
    squash.set_root_gid(0);
    walk(appdir, appdir, &mut squash, mtime);
    let mut image = Cursor::new(Vec::new());
    squash.write(&mut image).expect("squashfs");
    let mut file = fs::File::create(out).expect("output");
    file.write_all(&runtime(runtime_path)).unwrap();
    file.write_all(image.get_ref()).unwrap();
    drop(file);
    fs::set_permissions(out, fs::Permissions::from_mode(0o755)).unwrap();
    println!("{}: {} bytes", out.display(), fs::metadata(out).unwrap().len());
}
