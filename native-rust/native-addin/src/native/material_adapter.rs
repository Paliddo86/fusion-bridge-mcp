use super::*;
use crate::materials::{Backend, Library, Material, Result};
extern "C" {
    fn af_material_library_count(c: *mut c_void, d: *mut c_void, out: *mut u64) -> i32;
    fn af_material_library_read(
        c: *mut c_void,
        d: *mut c_void,
        index: u64,
        count: *mut u64,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_material_count(
        c: *mut c_void,
        d: *mut c_void,
        id: *const c_char,
        len: usize,
        out: *mut u64,
    ) -> i32;
    fn af_material_read(
        c: *mut c_void,
        d: *mut c_void,
        id: *const c_char,
        len: usize,
        index: u64,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
}
pub(super) struct Adapter<'a> {
    pub context: &'a Context,
    pub document: &'a DocumentReference,
}
impl Adapter<'_> {
    fn guard(&self) -> Result<()> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()
    }
    fn status(&self, stage: &str, code: i32) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(format!("native_material_sdk_{stage}_status_{code}"))
        }
    }
}
fn count(n: u64) -> Result<usize> {
    usize::try_from(n).map_err(|_| "native_material_count_overflow".into())
}
impl Backend for Adapter<'_> {
    fn library_count(&self) -> Result<usize> {
        self.guard()?;
        let mut out = 0;
        self.status("library_count", unsafe {
            af_material_library_count(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut out,
            )
        })?;
        count(out)
    }
    fn library(&self, index: usize) -> Result<Library> {
        self.guard()?;
        let mut fields = Fields::default();
        let mut n = 0;
        self.status("library", unsafe {
            af_material_library_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                index as u64,
                &mut n,
                collect,
                (&mut fields as *mut Fields).cast(),
            )
        })?;
        Ok(Library {
            id: field(&mut fields, 0)?,
            name: field(&mut fields, 1)?,
            count: count(n)?,
        })
    }
    fn material_count(&self, id: Option<&str>) -> Result<usize> {
        self.guard()?;
        let id = id.unwrap_or("");
        let mut n = 0;
        self.status("material_count", unsafe {
            af_material_count(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                id.as_ptr().cast(),
                id.len(),
                &mut n,
            )
        })?;
        count(n)
    }
    fn material(&self, id: Option<&str>, index: usize) -> Result<Material> {
        self.guard()?;
        let id = id.unwrap_or("");
        let mut fields = Fields::default();
        self.status("material", unsafe {
            af_material_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                id.as_ptr().cast(),
                id.len(),
                index as u64,
                collect,
                (&mut fields as *mut Fields).cast(),
            )
        })?;
        Ok(Material {
            id: field(&mut fields, 0)?,
            name: field(&mut fields, 1)?,
        })
    }
}
