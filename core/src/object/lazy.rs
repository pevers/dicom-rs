use std::cell::{Ref, RefMut, RefCell};
use std::collections::HashMap;
use std::io::{Seek, SeekFrom};
use std::fmt;
use std::fmt::Debug;
use std::rc::Rc;
use data::Header;
use dictionary::{DataDictionary, DictionaryEntry};
use data::parser::Parse;
use error::{Result, Error};
use data::{Tag, VR};
use data::value::DicomValue;
use iterator::DicomElementMarker;
use object::pixeldata::PixelData;
use util::ReadSeek;
use super::DicomObject;

/// Data type for a lazily loaded DICOM object builder.
pub struct LazyDicomObject<S, P, D> {
    dict: D,
    source: RefCell<S>,
    parser: P,
    entries: RefCell<HashMap<Tag, LazyDataElement>>,
}

impl<S, P, D> Debug for LazyDicomObject<S, P, D>
    where S: Debug,
          P: Debug,
          D: Debug
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "LazyDicomObject{{parser: {:?}, entries{:?}}}",
               &self.parser,
               &self.entries)
    }
}

impl<'s, S: 's, P, D> DicomObject<'s> for LazyDicomObject<S, P, D>
    where S: ReadSeek,
          P: Parse<S>,
          D: DataDictionary
{
    type Element = Ref<'s, LazyDataElement>;
    type Sequence = (); // TODO

    fn element(&'s self, tag: Tag) -> Result<Self::Element> {
        {
            let borrow = self.entries.borrow();
            if !borrow.contains_key(&tag) {
                return Err(Error::NoSuchDataElement);
            }
            let e = Ref::map(borrow, |m| m.get(&tag).expect("Element should exist"));
            if e.is_loaded() {
                return Ok(e);
            }
        }
        {
            let mut borrow = self.entries.borrow_mut();
            let mut e = borrow.get_mut(&tag).expect("Element should exist");
            let v: DicomValue = self.load_value(&e.marker).unwrap();
            let mut data = e.value_mut();
            *data = Some(v);
        }
        Ok(Ref::map(self.entries.borrow(),
                    |m| m.get(&tag).expect("Element should exist")))
    }

    fn element_by_name(&'s self, name: &str) -> Result<Self::Element> {
        let tag = self.lookup_name(name)?;
        self.element(tag)
    }

    fn pixel_data<PV, PX: PixelData<PV>>(&self) -> Result<PX> {
        unimplemented!()
    }
}

impl<S, P, D> LazyDicomObject<S, P, D>
    where S: ReadSeek,
          P: Parse<S>,
          D: DataDictionary
{
    fn lookup_name(&self, name: &str) -> Result<Tag> {
        self.dict
            .get_by_name(name)
            .ok_or(Error::NoSuchAttributeName)
            .map(|e| e.tag())
    }

    fn load_value(&self, marker: &DicomElementMarker) -> Result<DicomValue> {
        let mut borrow = self.source.borrow_mut();
        marker.move_to_start(borrow)?;
        self.parser.read_value(&marker.header)
    }
}

/// A data element containing the value only after the first read.
/// This element makes no further assumptions of where the
/// element really comes from, and cannot retrieve the value by itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LazyDataElement {
    marker: DicomElementMarker,
    value: Option<DicomValue>,
}

impl Header for LazyDataElement {
    fn tag(&self) -> Tag {
        self.marker.tag()
    }
    fn len(&self) -> u32 {
        self.marker.len()
    }
}

impl<'a> Header for &'a LazyDataElement {
    fn tag(&self) -> Tag {
        (**self).tag()
    }
    fn len(&self) -> u32 {
        (**self).len()
    }
}

impl<'s> Header for Ref<'s, LazyDataElement> {
    fn tag(&self) -> Tag {
        (**self).tag()
    }
    fn len(&self) -> u32 {
        (**self).len()
    }
}

impl Header for Rc<LazyDataElement> {
    fn tag(&self) -> Tag {
        (**self).tag()
    }
    fn len(&self) -> u32 {
        (**self).len()
    }
}

impl LazyDataElement {
    /// Create a new lazy element with the given marker.
    pub fn new(marker: DicomElementMarker) -> LazyDataElement {
        LazyDataElement {
            marker: marker,
            value: None,
        }
    }

    /// Retrieve the element's tag as a `(group, element)` tuple.
    pub fn tag(&self) -> Tag {
        self.marker.tag()
    }

    /// Retrieve the element's value representation, which can be unknown if
    /// not applicable.
    pub fn vr(&self) -> VR {
        self.marker.vr()
    }

    /// Retrieve the value data's length as specified by the data element.
    /// According to the standard, this can be 0xFFFFFFFFu32 if the length is undefined,
    /// which can be the case for sequence elements.
    pub fn len(&self) -> u32 {
        self.marker.len()
    }

    /// Getter for this element's cached data value.
    /// It will only hold a value once explicitly read.
    pub fn value(&self) -> Option<&DicomValue> {
        self.value.as_ref()
    }

    /// Mutable getter for this element's cached data container.
    pub fn value_mut(&mut self) -> &mut Option<DicomValue> {
        &mut self.value
    }

    pub fn is_loaded(&self) -> bool {
        self.value.is_some()
    }

    pub fn clear_value(&mut self) {
        self.value = None;
    }
}
