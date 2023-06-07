use std::ffi::{CStr, CString};

use anyhow::{bail, Result};
use libc::{c_char, c_int, c_uint, major, minor, stat, statvfs, FILE};

#[repr(C)]
struct extmnttab {
    mnt_special: *mut c_char,
    mnt_mountp: *mut c_char,
    mnt_fstype: *mut c_char,
    mnt_mntopts: *mut c_char,
    mnt_time: *mut c_char,
    mnt_major: c_uint,
    mnt_minor: c_uint,
}

#[allow(unused)]
#[link(name = "c")]
extern "C" {
    fn resetmnttab(fp: *mut FILE);
    fn getextmntent(fp: *mut FILE, mp: *mut extmnttab, len: c_int) -> c_int;
}

#[allow(unused)]
#[derive(Debug)]
struct MntTabEnt {
    special: String,
    mountp: String,
    fstype: String,
    mntopts: String,
    time: String,
    major: u32,
    minor: u32,
}

impl MntTabEnt {
    fn getopt(&self, name: &str) -> Option<String> {
        self.mntopts
            .split(',')
            .filter_map(|t| {
                if let Some((k, v)) = t.split_once('=') {
                    if k == name {
                        Some(v.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .next()
    }
}

fn last_errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn get_mnttab_ents() -> Result<Vec<MntTabEnt>> {
    let mut out = Vec::new();

    let path = CStr::from_bytes_with_nul(b"/etc/mnttab\0").unwrap();
    let mode = CStr::from_bytes_with_nul(b"r\0").unwrap();

    let f = unsafe { libc::fopen(path.as_ptr(), mode.as_ptr()) };
    if f.is_null() {
        bail!("open mnttab: {}", last_errno());
    }

    loop {
        let mut mp: extmnttab = unsafe { std::mem::zeroed() };

        let r = unsafe {
            getextmntent(f, &mut mp, std::mem::size_of::<extmnttab>() as i32)
        };

        if r < 0 {
            /*
             * EOF.
             */
            break;
        } else if r > 0 {
            /*
             * Error of some kind.
             */
            unsafe { libc::fclose(f) };
            bail!("getextmntent error {r}");
        }

        let special = unsafe { CStr::from_ptr(mp.mnt_special) }
            .to_str()
            .unwrap()
            .to_string();
        let mountp = unsafe { CStr::from_ptr(mp.mnt_mountp) }
            .to_str()
            .unwrap()
            .to_string();
        let fstype = unsafe { CStr::from_ptr(mp.mnt_fstype) }
            .to_str()
            .unwrap()
            .to_string();
        let mntopts = unsafe { CStr::from_ptr(mp.mnt_mntopts) }
            .to_str()
            .unwrap()
            .to_string();
        let time = unsafe { CStr::from_ptr(mp.mnt_time) }
            .to_str()
            .unwrap()
            .to_string();
        let major = mp.mnt_major;
        let minor = mp.mnt_minor;

        out.push(MntTabEnt {
            special,
            mountp,
            fstype,
            mntopts,
            time,
            major,
            minor,
        });
    }

    unsafe { libc::fclose(f) };

    Ok(out)
}

fn main() -> Result<()> {
    let a = getopts::Options::new()
        .parsing_style(getopts::ParsingStyle::StopAtFirstFree)
        .parse(std::env::args().skip(1))?;

    /*
     * Fetch information from mnttab(5) using getextmntent(3C) for all mounted
     * file systems:
     */
    let ents = get_mnttab_ents()?;

    for arg in a.free.iter() {
        let path = CString::new(arg.clone())?;
        let mut f: statvfs = unsafe { std::mem::zeroed() };
        let res = unsafe { statvfs(path.as_ptr(), &mut f) };
        if res != 0 {
            bail!("statvfs({path:?}) failed: {}", last_errno());
        }

        /*
         * Get the f_basetype string ready for comparison:
         */
        let basetype = unsafe { CStr::from_ptr(f.f_basetype.as_ptr()) };
        let basetype = basetype.to_str().unwrap();
        println!("f_basetype = {basetype:?}");

        /*
         * Curiously, and possibly as an accident of history, ZFS snapshots are
         * not shown as read-only in file system flags returned by vfsstat!  One
         * could argue that this is a bug which we should go and fix.
         */
        if (f.f_flag & libc::ST_RDONLY) != 0 {
            println!("read only!");
        } else {
            println!("NOT read only!");
        }

        /*
         * The fsid value is, today, the 32-bit compressed version of the unique
         * device ID that ZFS created for the file system or snapshot in
         * question.  These IDs are ephemeral for the current import of the ZFS
         * pool in question, but can be used to distinguish one dataset or
         * snapshot from another.
         *
         * Curiously, in the snapshot case, the vfsstat(2) f_fsid value reflects
         * the unique device number of the _file system_ rather than the
         * snapshot itself.  Unfortunately because it is a compressed device ID,
         * and this is a 64-bit system, I do not believe there is any public
         * function that allows us to expand back to a native width device ID.
         * Fortunately, the "dev" mount option is _also_ a compressed device, so
         * for now we'll just compare that string to this number.
         */
        println!("f_fsid -> {:x}", f.f_fsid);

        let mut st: stat = unsafe { std::mem::zeroed() };
        let res = unsafe { stat(path.as_ptr(), &mut st) };
        if res != 0 {
            bail!("stat({:?}) failed: {}", a.free[0], last_errno());
        }

        println!("st_dev = {:x}", st.st_dev);

        let fs_major = unsafe { major(st.st_dev) };
        let fs_minor = unsafe { minor(st.st_dev) };
        println!("st_dev fs_major -> {fs_major}");
        println!("st_dev fs_minor -> {fs_minor}");

        /*
         * Get the st_fstype string ready for comparison and confirm
         * it matches what we got from vfsstat(2).
         *
         * The libc crate "struct stat" definition has made a curious
         * decision to make "st_fstype" into a _private_ field named
         * __unused.  It's hard to understand why such a hostile decision
         * has arisen, but in the mean time it is still our computer, so:
         */
        let fstypeaddr =
            ((std::ptr::addr_of!(st) as usize) + 0x70) as *const c_char;

        let st_fstype = unsafe { CStr::from_ptr(fstypeaddr) };
        let st_fstype = st_fstype.to_str().unwrap();
        println!("st_fstype = {st_fstype:?}");

        if st_fstype != basetype {
            bail!("st_fstype {st_fstype:?} != f_basetype {basetype:?}");
        }

        #[derive(Debug)]
        enum WhatIsIt {
            Snapshot(String),
            Live(String),
        }

        /*
         * Look for a mnttab entry that matches.
         */
        let mut mats = ents.iter().filter_map(|ent| {
            /*
             * We need to make sure the file system base type (a name, like
             * "zfs") matches the values we read earlier.  We also want to
             * confirm, then, that the major number for the device is the same
             * as the one we saw before.  For ZFS, this major number is the same
             * for all pools, and does not reflect any underlying block storage
             * device drivers.
             */
            if ent.fstype != basetype || fs_major != ent.major {
                return None;
            }

            /*
             * Does this mount entry match the file system device ID we got?
             */
            if ent.getopt("dev") != Some(format!("{:x}", f.f_fsid)) {
                return None;
            }

            if fs_minor != ent.minor {
                /*
                 * If the file system device minor number does not match the
                 * mount entry, but the fsid does, this is a snapshot of that
                 * file system.
                 */
                Some(WhatIsIt::Snapshot(ent.special.to_string()))
            } else {
                /*
                 * Otherwise, this is the live file system.
                 */
                Some(WhatIsIt::Live(ent.special.to_string()))
            }
        });

        let mat = mats.next();
        if let Some(mat) = mat {
            if let Some(another) = mats.next() {
                bail!("two matches? {mat:?} and {another:?}");
            }

            match mat {
                WhatIsIt::Snapshot(fs) => {
                    println!("snapshot of:      {fs}")
                }
                WhatIsIt::Live(fs) => println!("live file system: {fs}"),
            }
        } else {
            bail!("no match found?");
        }

        println!();
    }

    Ok(())
}
