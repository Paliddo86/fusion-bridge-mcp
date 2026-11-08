use std::ffi::{c_char, c_void};

#[derive(Default)]
pub(crate) struct Fields {
    values: [Option<String>; 5],
    failed: bool,
}

// The shim guarantees that non-null data points to len readable bytes for this
// synchronous callback only. Copy now; no borrowed SDK bytes survive the call.
pub(crate) unsafe extern "C" fn collect(
    user: *mut c_void,
    field: u32,
    data: *const c_char,
    len: usize,
) -> i32 {
    if user.is_null() {
        return 0;
    }
    let fields = &mut *user.cast::<Fields>();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = field as usize;
        if fields.failed
            || index >= 5
            || fields.values[index].is_some()
            || len > 1_048_576
            || (data.is_null() && len != 0)
        {
            return false;
        }
        let bytes = if len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(data.cast::<u8>(), len)
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let mut owned = String::new();
        if owned.try_reserve_exact(text.len()).is_err() {
            return false;
        }
        owned.push_str(text);
        fields.values[index] = Some(owned);
        true
    }))
    .unwrap_or(false);
    fields.failed |= !result;
    i32::from(result)
}

pub(crate) fn field(fields: &mut Fields, index: usize) -> Result<String, String> {
    if fields.failed {
        return Err("SDK byte transfer failed".into());
    }
    fields
        .values
        .get_mut(index)
        .and_then(Option::take)
        .ok_or_else(|| format!("missing SDK field {index}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn send(fields: &mut Fields, id: u32, bytes: &[u8]) -> i32 {
        unsafe {
            collect(
                (fields as *mut Fields).cast(),
                id,
                bytes.as_ptr().cast(),
                bytes.len(),
            )
        }
    }
    #[test]
    fn copies_utf8_without_a_borrowed_sdk_lifetime() {
        let mut fields = Fields::default();
        let mut bytes = "轴承 ⚙".as_bytes().to_vec();
        assert_eq!(send(&mut fields, 0, &bytes), 1);
        bytes.fill(0);
        assert_eq!(field(&mut fields, 0).unwrap(), "轴承 ⚙");
    }
    #[test]
    fn duplicate_or_unknown_field_poison_the_transfer() {
        for bad in [0, 5, u32::MAX] {
            let mut fields = Fields::default();
            assert_eq!(send(&mut fields, 0, b"ok"), 1);
            assert_eq!(send(&mut fields, bad, b"bad"), 0);
            assert!(field(&mut fields, 0).is_err());
            assert_eq!(send(&mut fields, 1, b"ignored"), 0);
        }
    }
    #[test]
    fn malformed_utf8_is_not_lossily_replaced() {
        let mut fields = Fields::default();
        assert_eq!(send(&mut fields, 0, &[0xff]), 0);
        assert!(field(&mut fields, 0).is_err());
    }
    #[test]
    fn null_and_oversized_spans_refuse_before_dereference() {
        for len in [1, 1_048_577, usize::MAX] {
            let mut fields = Fields::default();
            assert_eq!(
                unsafe {
                    collect(
                        (&mut fields as *mut Fields).cast(),
                        0,
                        std::ptr::null(),
                        len,
                    )
                },
                0
            );
        }
        assert_eq!(
            unsafe { collect(std::ptr::null_mut(), 0, std::ptr::null(), 0) },
            0
        );
    }
    #[test]
    fn empty_and_missing_are_distinct() {
        let mut fields = Fields::default();
        assert_eq!(send(&mut fields, 0, b""), 1);
        assert_eq!(field(&mut fields, 0).unwrap(), "");
        assert!(field(&mut fields, 1).is_err());
    }
}
