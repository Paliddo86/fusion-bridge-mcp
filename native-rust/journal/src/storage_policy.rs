//! Deliberately narrow ACL grammar, shared by Win32 inspection and portable tests.
//! Trusted administrators and SYSTEM can already override local user isolation.
//! No other principal receives even read access; deny/conditional/object ACEs
//! are unsupported rather than approximated by an effective-access calculation.

pub(super) const FULL_CONTROL: u32 = 0x001f_01ff;
const GENERIC_ALL: u32 = 0x1000_0000;
const KNOWN_RIGHTS: u32 = FULL_CONTROL | 0xf000_0000;
pub(super) const OBJECT_INHERIT: u8 = 1;
pub(super) const CONTAINER_INHERIT: u8 = 2;
const INHERITED: u8 = 0x10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Principal {
    User,
    System,
    Administrators,
    Other,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Ace {
    pub principal: Principal,
    pub allow: bool,
    pub flags: u8,
    pub mask: u32,
}

pub(super) fn private_acl(
    owner_is_user: bool,
    protected: bool,
    directory: bool,
    aces: Option<&[Ace]>,
) -> bool {
    if !owner_is_user || (directory && !protected) {
        return false;
    }
    let Some(aces) = aces else { return false };
    let mut user_full_control = false;
    for ace in aces {
        if !ace.allow
            || ace.principal == Principal::Other
            || ace.flags & !(OBJECT_INHERIT | CONTAINER_INHERIT | INHERITED) != 0
            || ace.mask & !KNOWN_RIGHTS != 0
        {
            return false;
        }
        if ace.principal == Principal::User
            && (ace.mask & FULL_CONTROL == FULL_CONTROL || ace.mask & GENERIC_ALL != 0)
            && (!directory
                || ace.flags & (OBJECT_INHERIT | CONTAINER_INHERIT)
                    == OBJECT_INHERIT | CONTAINER_INHERIT)
        {
            user_full_control = true;
        }
    }
    user_full_control
}

pub(super) fn supported_filesystem(
    name: &str,
    fixed: bool,
    persistent_acl: bool,
    hardlinks: bool,
) -> bool {
    name == "NTFS" && fixed && persistent_acl && hardlinks
}

#[cfg(test)]
mod tests {
    use super::*;
    fn user() -> Ace {
        Ace {
            principal: Principal::User,
            allow: true,
            flags: 3,
            mask: FULL_CONTROL,
        }
    }
    #[test]
    fn private_acl_requires_owner_real_dacl_and_effective_inheritable_user_access() {
        let full = [user()];
        assert!(private_acl(true, true, true, Some(&full)));
        assert!(!private_acl(false, true, true, Some(&full)));
        assert!(!private_acl(true, false, true, Some(&full)));
        assert!(!private_acl(true, true, true, None));
        assert!(!private_acl(true, true, true, Some(&[])));
        for flags in [0, 1, 2, 3 | 4, 3 | 8, 3 | 0x20, 3 | 0x40, 3 | 0x80] {
            assert!(!private_acl(
                true,
                true,
                true,
                Some(&[Ace { flags, ..user() }])
            ));
        }
        assert!(private_acl(
            true,
            false,
            false,
            Some(&[Ace {
                flags: INHERITED,
                ..user()
            }])
        ));
        assert!(!private_acl(
            true,
            true,
            false,
            Some(&[Ace {
                mask: 0x120089,
                ..user()
            }])
        ));
    }
    #[test]
    fn private_acl_rejects_public_read_deny_unknown_rights_and_trusts_only_named_admins() {
        for principal in [Principal::System, Principal::Administrators] {
            assert!(private_acl(
                true,
                true,
                true,
                Some(&[
                    user(),
                    Ace {
                        principal,
                        ..user()
                    }
                ])
            ));
            assert!(!private_acl(
                true,
                true,
                true,
                Some(&[Ace {
                    principal,
                    ..user()
                }])
            ));
        }
        for forbidden in [
            Ace {
                principal: Principal::Other,
                mask: 0x120089,
                ..user()
            },
            Ace {
                principal: Principal::Other,
                mask: 0,
                ..user()
            },
            Ace {
                allow: false,
                ..user()
            },
            Ace {
                mask: FULL_CONTROL | 0x0100_0000,
                ..user()
            },
        ] {
            assert!(!private_acl(true, true, true, Some(&[user(), forbidden])));
        }
    }
    #[test]
    fn filesystem_policy_excludes_refs_remote_and_missing_durable_primitives() {
        assert!(supported_filesystem("NTFS", true, true, true));
        for name in ["ReFS", "FAT32", "exFAT", "ntfs", ""] {
            assert!(!supported_filesystem(name, true, true, true));
        }
        assert!(!supported_filesystem("NTFS", false, true, true));
        assert!(!supported_filesystem("NTFS", true, false, true));
        assert!(!supported_filesystem("NTFS", true, true, false));
    }
}
