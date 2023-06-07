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
        println!("looking at path: {path:?}");

        /*
         * Perform a statvfs(2) call against the path.  Though we are providing
         * a path that is potentially not the mount point of any particular file
         * system, the call will determine which file system the file resides
         * in.
         *
         * Even though we have nominated a file, this call is specific to the
         * file system.  The process of automatically mounting snapshots under
         * ".zfs/snapshot/NAME/..." is somewhat magical and does not result in a
         * visible mount entry for the snapshot; see zfsctl_snapdir_lookup().
         * Some of this magic is in service of NFS exports of ZFS file systems,
         * to make the snapshot appear as effectively just a regular directory
         * and thus not require an explicit and separate NFS mount on the client
         * to cross into the snapdir.  Nonetheless, this also serves our
         * purposes here.
         *
         * This call will end up telling us details about the live file system,
         * even if we hit a snapshot.
         */
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
         * The fsid value is, today, the 32-bit compressed version of the unique
         * device ID that ZFS created for the file system in question.  These
         * IDs are ephemeral for the current import of the ZFS pool in question,
         * but can be used to distinguish one dataset or snapshot from another.
         *
         * Unfortunately because it is a compressed device ID and this is a
         * 64-bit system, I do not believe there is any public function that
         * allows us to expand back to a native width device ID.  Fortunately,
         * the "dev" mount option is _also_ expressed as a compressed device, so
         * for now we'll just compare that string to this number.
         */
        println!("f_fsid = 0x{:x}", f.f_fsid);

        /*
         * Now, make a stat(2) call against the path.  This call _is_
         * vnode-specific, and thus some of the information we get will be
         * lifted from the snapshot if this is one.
         */
        let mut st: stat = unsafe { std::mem::zeroed() };
        let res = unsafe { stat(path.as_ptr(), &mut st) };
        if res != 0 {
            bail!("stat({:?}) failed: {}", a.free[0], last_errno());
        }

        /*
         * The device number is, again, ephemeral to this import but unique on
         * the system at any given moment.  Snapshots get their own device
         * numbers, which are visible through stat(2).  See
         * zfs_create_unique_device().
         */
        println!("st_dev = 0x{:x}", st.st_dev);

        let fs_major = unsafe { major(st.st_dev) };
        let fs_minor = unsafe { minor(st.st_dev) };
        println!("st_dev major() -> {fs_major} (0x{fs_major:x})");
        println!("st_dev minor() -> {fs_minor} (0x{fs_minor:x})");

        /*
         * Get the st_fstype string ready for comparison and confirm it matches
         * what we got from statvfs(2).
         *
         * The libc crate has made a curious decision to make "st_fstype" into a
         * _private_ field named __unused.  It's hard to understand why such a
         * hostile situation has arisen, but in the mean time it is in fact our
         * computer:
         */
        let fstypeaddr =
            ((std::ptr::addr_of!(st) as usize) + 0x70) as *const c_char;

        let st_fstype = unsafe { CStr::from_ptr(fstypeaddr) };
        let st_fstype = st_fstype.to_str().unwrap();

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
             * This ID is for the live file system, whether or not we were
             * looking at a snapshot, so it must match something visible in the
             * mount table.
             */
            if ent.getopt("dev") != Some(format!("{:x}", f.f_fsid)) {
                return None;
            }

            if fs_minor != ent.minor {
                /*
                 * If the file system device minor number from stat(2) does not
                 * match the mount entry, but the statvfs(2) fsid does, this is
                 * a snapshot of that file system.
                 */
                Some(WhatIsIt::Snapshot(ent.special.to_string()))
            } else {
                /*
                 * Otherwise, if everything matches, this is the live file
                 * system itself.
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
