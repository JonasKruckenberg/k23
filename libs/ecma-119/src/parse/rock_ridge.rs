use bitflags::bitflags;
use fallible_iterator::FallibleIterator;

use crate::parse::parser::Parser;
use crate::parse::susp::SystemUseIter;
use crate::{BothEndianU32, DirDateTime, ParseError, SystemUseEntry};

#[derive(Debug)]
pub enum RockRidgeEntry<'a> {
    AlternateName(AlternateName<'a>),
    Attributes(PosixAttributes),
    Timestamps(PosixTimestamp<'a>),
    Unknown(&'a str),
}

#[derive(Debug)]
struct PosixAttributes {
    /// Equivalent to the POSIX.1 `st_mode` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub mode: PosixFileMode,

    /// Equivalent to the POSIX.1 `st_nlink` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub links: u32,

    /// Equivalent to the POSIX.1 `st_uid` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub uid: u32,

    /// Equivalent to the POSIX.1 `st_gid` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub gid: u32,

    /// Serial number. Equivalent to the POSIX.1 `st_ino` field.
    ///
    /// ## Notes
    ///
    /// This was introduced in Rock Ridge v1.12 and may not be present.
    pub inode: Option<u32>,
}

#[derive(Debug)]
pub struct PosixTimestamp<'a> {
    /// POSIX.1 `st_atime`
    pub access: Option<&'a DirDateTime>,
    /// POSIX.1 `st_ctime`
    pub attributes: Option<&'a DirDateTime>,
    /// The "backup" time.  Its use is essentially undefined.
    pub backup: Option<&'a DirDateTime>,
    /// ISO 9660 / ECMA-119 § 9.5.4
    pub creation: Option<&'a DirDateTime>,
    /// ISO 9660 / ECMA-119 § 9.5.7
    pub effective: Option<&'a DirDateTime>,
    /// ISO 9660 / ECMA-119 § 9.5.6
    pub expiration: Option<&'a DirDateTime>,
    /// POSIX.1 `st_mtime`, ISO 9660 / ECMA-119 § 9.5.5
    pub modify: Option<&'a DirDateTime>,
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    struct PosixTimestampFlags: u8 {
        const CREATION = 1 << 0;
        const MODIFY = 1 << 1;
        const ACCESS = 1 << 2;
        const ATTRIBUTES = 1 << 3;
        const BACKUP = 1 << 4;
        const EXPIRATION = 1 << 5;
        const EFFECTIVE = 1 << 6;
        const LONG_FORM = 1 << 7;
    }
}

bitflags! {
    /// The mode from a `PX` entry.  Equivalent to POSIX.1's `st_mode` field.
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub struct PosixFileMode: u32 {
        /// Directory entry is an `AF_LOCAL` (née `AF_UNIX`) socket.  Equivalent to `S_IFSOCK`.
        const TYPE_SOCKET    = 0o0140000;

        /// Directory entry is a symbolic link.  This indicates an `SL` entry should also be present.  Equivalent to X/Open System Interfaces (XSI) `S_IFLNK` and POSIX.1 `S_ISSOCK()`.
        const TYPE_SYMLINK   = 0o0120000;

        /// Directory entry is a regular file.  Equivalent to X/Open System Interfaces (XSI) `S_IFREG` and POSIX.1 `S_ISREG()`.
        const TYPE_FILE      = 0o0100000;

        /// Directory entry is a block device.  Equivalent to X/Open System Interfaces (XSI) `S_IFBLK` and POSIX.1 `S_ISBLK()`.
        const TYPE_BLOCK_DEV = 0o0060000;

        /// Directory entry is a directory.  Equivalent to X/Open System Interfaces (XSI) `S_IFDIR` and POSIX.1 `S_ISDIR()`.
        const TYPE_DIRECTORY = 0o0040000;

        /// Directory entry is a character device.  Equivalent to X/Open System Interfaces (XSI) `S_IFCHR` and POSIX.1 `S_ISCHR()`.
        const TYPE_CHAR_DEV  = 0o0020000;

        /// Directory entry is a named pipe.  Equivalent to X/Open System Interfaces (XSI) `S_IFIFO` and POSIX.1 `S_ISFIFO()`.
        const TYPE_PIPE      = 0o0010000;

        /// If executed, will be run with the UID of the file's owner.  Equivalent to POSIX.1 `S_ISUID`.
        const SET_UID        = 0o0004000;

        /// If executed, will be run with the GID of the file's owner.  Equivalent to POSIX.1 `S_ISGID`.
        const SET_GID        = 0o0002000;

        /// This needs to go away.
        const LOCKABLE       = 0o0002000;

        /// Sticky bit, or lots of legacy cruft.  Your choice.  Equivalent to X/Open System Interfaces (XSI) `S_ISVTX` and BSD `S_ISTXT`.
        const STICKY         = 0o0001000;

        /// Equivalent to POSIX.1 `S_IRUSR`.
        const OWN_READ       = 0o0000400;

        /// Equivalent to POSIX.1 `S_IWUSR`.
        const OWN_WRITE      = 0o0000200;

        /// Equivalent to POSIX.1 `S_IXUSR`.
        const OWN_EXEC       = 0o0000100;

        /// Equivalent to POSIX.1 `S_IRGRP`.
        const GROUP_READ     = 0o0000040;

        /// Equivalent to POSIX.1 `S_IWGRP`.
        const GROUP_WRITE    = 0o0000020;

        /// Equivalent to POSIX.1 `S_IXGRP`.
        const GROUP_EXEC     = 0o0000010;

        /// Equivalent to POSIX.1 `S_IROTH`.
        const WORLD_READ     = 0o0000004;

        /// Equivalent to POSIX.1 `S_IWOTH`.
        const WORLD_WRITE    = 0o0000002;

        /// Equivalent to POSIX.1 `S_IXOTH`.
        const WORLD_EXEC     = 0o0000001;

        /// Equivalent to POSIX.1 `S_ISUID | S_ISGID | S_IRWXU | S_IRWXG | S_IRWXO`
        const ALL_PERMISSIONS = Self::OWN_READ.bits() | Self::OWN_WRITE.bits() | Self::OWN_EXEC.bits() | Self::SET_UID.bits() |
                                Self::GROUP_READ.bits() | Self::GROUP_WRITE.bits() | Self::GROUP_EXEC.bits() | Self::SET_GID.bits() |
                                Self::WORLD_READ.bits() | Self::WORLD_WRITE.bits() | Self::WORLD_EXEC.bits();
    }
}

#[derive(Debug)]
pub(crate) struct AlternateName<'a> {
    pub name: &'a str,
    pub flags: AlternateNameFlags,
}

bitflags! {
    #[derive(Debug)]
    pub struct AlternateNameFlags: u8 {
        const CONTINUE = 1 << 0;
        const CURRENT = 1 << 1;
        const PARENT = 1 << 2;
        // 3
        // 4
        const HOST = 1 << 5;
        // 6
        // 7
    }
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    pub struct SymbolicLinkRecordFlags: u8 {
        const CONTINUE = 1 << 0;
        const CURRENT = 1 << 1;
        const PARENT = 1 << 2;
        const ROOT = 1 << 3;
        const VOLUME_ROOT = 1 << 4;
        const HOSTNAME = 1 << 5;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolicLinkRecord<'a> {
    pub flags: SymbolicLinkRecordFlags,
    pub component: &'a str,
}

pub struct RockRidgeIter<'a> {
    pub(super) inner: SystemUseIter<'a>,
}

impl<'a> FallibleIterator for RockRidgeIter<'a> {
    type Item = RockRidgeEntry<'a>;

    type Error = ParseError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(entry) = self.inner.next()? else {
            return Ok(None);
        };

        let SystemUseEntry::Unknown { header, data } = entry else {
            panic!("unexpected rock ridge entry")
        };

        let mut parser = Parser::new(data);

        match &header.signature {
            b"PX" => {
                assert_eq!(header.version, 1);
                let with_inode = match header.len {
                    44 => true,  // with inode
                    36 => false, // without inode
                    _ => panic!(),
                };

                let mode = parser.read_validated::<BothEndianU32>()?;
                let links = parser.read_validated::<BothEndianU32>()?;
                let uid = parser.read_validated::<BothEndianU32>()?;
                let gid = parser.read_validated::<BothEndianU32>()?;
                let inode = if with_inode {
                    Some(parser.read_validated::<BothEndianU32>()?)
                } else {
                    None
                };

                assert!(parser.pos == parser.data.len());

                Ok(Some(RockRidgeEntry::Attributes(PosixAttributes {
                    mode: PosixFileMode::from_bits(mode.get()).unwrap(),
                    links: links.get(),
                    uid: uid.get(),
                    gid: gid.get(),
                    inode: inode.map(|n| n.get()),
                })))
            }
            b"PN" => {
                // todo!("POSIX device number")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            b"SL" => {
                let flags = *parser.read::<u8>()?;

                let should_continue = match flags {
                    0x0 => true,
                    0x1 => false,
                    _ => todo!(),
                };

                // parser.

                // todo!("symbolic link")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            b"NM" => {
                assert_eq!(header.version, 1);

                let flags = AlternateNameFlags::from_bits(*parser.read::<u8>()?).unwrap();

                let name = str::from_utf8(parser.into_rest()).unwrap();

                Ok(Some(RockRidgeEntry::AlternateName(AlternateName {
                    name,
                    flags,
                })))
            }
            b"CL" => {
                // todo!("child link")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            b"PL" => {
                // todo!("parent link")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            b"RE" => {
                // todo!("relocated directory")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            b"TF" => {
                let flags = PosixTimestampFlags::from_bits(*parser.read::<u8>()?).unwrap();

                let creation = match flags.contains(PosixTimestampFlags::CREATION) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let modify = match flags.contains(PosixTimestampFlags::MODIFY) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let access = match flags.contains(PosixTimestampFlags::ACCESS) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let attributes = match flags.contains(PosixTimestampFlags::ATTRIBUTES) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let backup = match flags.contains(PosixTimestampFlags::BACKUP) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let expiration = match flags.contains(PosixTimestampFlags::EXPIRATION) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };
                let effective = match flags.contains(PosixTimestampFlags::EFFECTIVE) {
                    true => Some(parser.read_validated::<DirDateTime>()?),
                    false => None,
                };

                assert!(parser.pos == parser.data.len());

                Ok(Some(RockRidgeEntry::Timestamps(PosixTimestamp {
                    access,
                    attributes,
                    backup,
                    creation,
                    effective,
                    expiration,
                    modify,
                })))
            }
            b"SF" => {
                // todo!("sparse file")
                Ok(Some(RockRidgeEntry::Unknown(
                    str::from_utf8(&header.signature).unwrap(),
                )))
            }
            sig => panic!("unknowmn rock ridge entry {:?}", str::from_utf8(sig)),
        }
    }
}
