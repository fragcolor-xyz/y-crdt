pub mod array;
pub mod map;
pub mod text;
#[cfg(feature = "weak")]
pub mod weak;
pub mod xml;

use crate::*;
pub use map::Map;
pub use map::MapRef;
use std::borrow::Borrow;
pub use text::Text;
pub use text::TextRef;

use crate::block::{Item, ItemContent, ItemPosition, ItemPtr, Prelim};
use crate::encoding::read::Error;
use crate::store::WeakStoreRef;
use crate::transaction::{Origin, TransactionMut};
use crate::types::array::{ArrayEvent, ArrayRef};
use crate::types::map::MapEvent;
use crate::types::text::TextEvent;
#[cfg(feature = "weak")]
use crate::types::weak::{LinkSource, WeakEvent, WeakRef};
use crate::types::xml::{XmlElementRef, XmlEvent, XmlTextEvent, XmlTextRef};
use crate::updates::decoder::{Decode, Decoder};
use crate::updates::encoder::{Encode, Encoder};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::Arc;

/// Type ref identifier for an [ArrayRef] type.
pub const TYPE_REFS_ARRAY: u8 = 0;

/// Type ref identifier for a [MapRef] type.
pub const TYPE_REFS_MAP: u8 = 1;

/// Type ref identifier for a [TextRef] type.
pub const TYPE_REFS_TEXT: u8 = 2;

/// Type ref identifier for a [XmlElementRef] type.
pub const TYPE_REFS_XML_ELEMENT: u8 = 3;

/// Type ref identifier for a [XmlFragmentRef] type. Used for compatibility.
pub const TYPE_REFS_XML_FRAGMENT: u8 = 4;

/// Type ref identifier for a [XmlHookRef] type. Used for compatibility.
pub const TYPE_REFS_XML_HOOK: u8 = 5;

/// Type ref identifier for a [XmlTextRef] type.
pub const TYPE_REFS_XML_TEXT: u8 = 6;

/// Type ref identifier for a [WeakRef] type.
pub const TYPE_REFS_WEAK: u8 = 7;

/// Type ref identifier for a [DocRef] type.
pub const TYPE_REFS_DOC: u8 = 9;

/// Placeholder type ref identifier for non-specialized AbstractType. Used only for root-level types
/// which have been integrated from remote peers before they were defined locally.
pub const TYPE_REFS_UNDEFINED: u8 = 15;

#[repr(u8)]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypeRef {
    Array = TYPE_REFS_ARRAY,
    Map = TYPE_REFS_MAP,
    Text = TYPE_REFS_TEXT,
    XmlElement(Arc<str>) = TYPE_REFS_XML_ELEMENT,
    XmlFragment = TYPE_REFS_XML_FRAGMENT,
    XmlHook = TYPE_REFS_XML_HOOK,
    XmlText = TYPE_REFS_XML_TEXT,
    SubDoc = TYPE_REFS_DOC,
    #[cfg(feature = "weak")]
    WeakLink(Arc<LinkSource>) = TYPE_REFS_WEAK,
    Undefined = TYPE_REFS_UNDEFINED,
}

impl TypeRef {
    pub fn kind(&self) -> u8 {
        match self {
            TypeRef::Array => TYPE_REFS_ARRAY,
            TypeRef::Map => TYPE_REFS_MAP,
            TypeRef::Text => TYPE_REFS_TEXT,
            TypeRef::XmlElement(_) => TYPE_REFS_XML_ELEMENT,
            TypeRef::XmlFragment => TYPE_REFS_XML_FRAGMENT,
            TypeRef::XmlHook => TYPE_REFS_XML_HOOK,
            TypeRef::XmlText => TYPE_REFS_XML_TEXT,
            TypeRef::SubDoc => TYPE_REFS_DOC,
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(_) => TYPE_REFS_WEAK,
            TypeRef::Undefined => TYPE_REFS_UNDEFINED,
        }
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Array => write!(f, "Array"),
            TypeRef::Map => write!(f, "Map"),
            TypeRef::Text => write!(f, "Text"),
            TypeRef::XmlElement(name) => write!(f, "XmlElement({})", name),
            TypeRef::XmlFragment => write!(f, "XmlFragment"),
            TypeRef::XmlHook => write!(f, "XmlHook"),
            TypeRef::XmlText => write!(f, "XmlText"),
            TypeRef::SubDoc => write!(f, "Doc"),
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(_) => write!(f, "WeakRef"),
            TypeRef::Undefined => write!(f, "(undefined)"),
        }
    }
}

impl Encode for TypeRef {
    fn encode<E: Encoder>(&self, encoder: &mut E) {
        match self {
            TypeRef::Array => encoder.write_type_ref(TYPE_REFS_ARRAY),
            TypeRef::Map => encoder.write_type_ref(TYPE_REFS_MAP),
            TypeRef::Text => encoder.write_type_ref(TYPE_REFS_TEXT),
            TypeRef::XmlElement(name) => {
                encoder.write_type_ref(TYPE_REFS_XML_ELEMENT);
                encoder.write_key(&name);
            }
            TypeRef::XmlFragment => encoder.write_type_ref(TYPE_REFS_XML_FRAGMENT),
            TypeRef::XmlHook => encoder.write_type_ref(TYPE_REFS_XML_HOOK),
            TypeRef::XmlText => encoder.write_type_ref(TYPE_REFS_XML_TEXT),
            TypeRef::SubDoc => encoder.write_type_ref(TYPE_REFS_DOC),
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(data) => {
                let is_single = data.is_single();
                let start = data.quote_start.id().unwrap();
                let end = data.quote_end.id().unwrap();
                encoder.write_type_ref(TYPE_REFS_WEAK);
                let mut info = if is_single { 0u8 } else { 1u8 };
                info |= match data.quote_start.assoc {
                    Assoc::After => 2,
                    Assoc::Before => 0,
                };
                info |= match data.quote_end.assoc {
                    Assoc::After => 4,
                    Assoc::Before => 0,
                };
                encoder.write_u8(info);
                encoder.write_var(start.client);
                encoder.write_var(start.clock);
                if !is_single {
                    encoder.write_var(end.client);
                    encoder.write_var(end.clock);
                }
            }
            TypeRef::Undefined => encoder.write_type_ref(TYPE_REFS_UNDEFINED),
        }
    }
}

impl Decode for TypeRef {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, Error> {
        let type_ref = decoder.read_type_ref()?;
        match type_ref {
            TYPE_REFS_ARRAY => Ok(TypeRef::Array),
            TYPE_REFS_MAP => Ok(TypeRef::Map),
            TYPE_REFS_TEXT => Ok(TypeRef::Text),
            TYPE_REFS_XML_ELEMENT => Ok(TypeRef::XmlElement(decoder.read_key()?)),
            TYPE_REFS_XML_FRAGMENT => Ok(TypeRef::XmlFragment),
            TYPE_REFS_XML_HOOK => Ok(TypeRef::XmlHook),
            TYPE_REFS_XML_TEXT => Ok(TypeRef::XmlText),
            TYPE_REFS_DOC => Ok(TypeRef::SubDoc),
            #[cfg(feature = "weak")]
            TYPE_REFS_WEAK => {
                let flags = decoder.read_u8()?;
                let is_single = flags & 1u8 == 0;
                let start_assoc = if flags & 2 == 2 {
                    Assoc::After
                } else {
                    Assoc::Before
                };
                let end_assoc = if flags & 4 == 4 {
                    Assoc::After
                } else {
                    Assoc::Before
                };
                let start_id = ID::new(decoder.read_var()?, decoder.read_var()?);
                let end_id = if is_single {
                    start_id.clone()
                } else {
                    ID::new(decoder.read_var()?, decoder.read_var()?)
                };
                let start = StickyIndex::from_id(start_id, start_assoc);
                let end = StickyIndex::from_id(end_id, end_assoc);
                Ok(TypeRef::WeakLink(Arc::new(LinkSource::new(start, end))))
            }
            TYPE_REFS_UNDEFINED => Ok(TypeRef::Undefined),
            _ => Err(Error::UnexpectedValue),
        }
    }
}

pub trait Observable: AsRef<Branch> {
    type Event;

    /// Subscribes a given callback to be triggered whenever current y-type is changed.
    /// A callback is triggered whenever a transaction gets committed. This function does not
    /// trigger if changes have been observed by nested shared collections.
    ///
    /// All array-like event changes can be tracked by using [Event::delta] method.
    /// All map-like event changes can be tracked by using [Event::keys] method.
    /// All text-like event changes can be tracked by using [TextEvent::delta] method.
    ///
    /// Returns a [Subscription] which, when dropped, will unsubscribe current callback.
    fn observe<F>(&self, f: F) -> Subscription
    where
        F: Fn(&TransactionMut, &Self::Event) -> () + 'static,
        Event: AsRef<Self::Event>,
    {
        let mut branch = BranchPtr::from(self.as_ref());
        branch.observe(move |txn, e| {
            let mapped_event = e.as_ref();
            f(txn, mapped_event)
        })
    }
}

/// Trait implemented by shared types to display their contents in string format.
pub trait GetString {
    /// Displays the content of a current collection in string format.
    fn get_string<T: ReadTxn>(&self, txn: &T) -> String;
}

pub trait SharedRef: From<BranchPtr> + AsRef<Branch> {}

/// A wrapper around [Branch] cell, supplied with a bunch of convenience methods to operate on both
/// map-like and array-like contents of a [Branch].
#[repr(transparent)]
#[derive(Clone, Copy, Hash)]
pub struct BranchPtr(NonNull<Branch>);

impl BranchPtr {
    pub(crate) fn trigger(
        &self,
        txn: &TransactionMut,
        subs: HashSet<Option<Arc<str>>>,
    ) -> Option<Event> {
        let e = self.make_event(subs)?;
        if let Some(callbacks) = self.observers.callbacks() {
            for fun in callbacks {
                fun(txn, &e);
            }
        }

        Some(e)
    }

    pub(crate) fn trigger_deep(&self, txn: &TransactionMut, e: &Events) {
        if let Some(callbacks) = self.deep_observers.callbacks() {
            for fun in callbacks {
                fun(txn, e);
            }
        }
    }
}

impl Into<TypePtr> for BranchPtr {
    fn into(self) -> TypePtr {
        TypePtr::Branch(self)
    }
}

impl Into<Origin> for BranchPtr {
    fn into(self) -> Origin {
        let addr = self.0.as_ptr() as usize;
        let bytes = addr.to_be_bytes();
        Origin::from(bytes.as_ref())
    }
}

impl AsRef<Branch> for BranchPtr {
    fn as_ref(&self) -> &Branch {
        self.deref()
    }
}

impl AsMut<Branch> for BranchPtr {
    fn as_mut(&mut self) -> &mut Branch {
        self.deref_mut()
    }
}

impl Deref for BranchPtr {
    type Target = Branch;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for BranchPtr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

impl<'a> From<&'a mut Arc<Branch>> for BranchPtr {
    fn from(branch: &'a mut Arc<Branch>) -> Self {
        let ptr = NonNull::from(branch.as_ref());
        BranchPtr(ptr)
    }
}

impl<'a> From<&'a Arc<Branch>> for BranchPtr {
    fn from(branch: &'a Arc<Branch>) -> Self {
        let b: &Branch = &*branch;

        let ptr = unsafe { NonNull::new_unchecked(b as *const Branch as *mut Branch) };
        BranchPtr(ptr)
    }
}

impl<'a> From<&'a Branch> for BranchPtr {
    fn from(branch: &'a Branch) -> Self {
        let ptr = unsafe { NonNull::new_unchecked(branch as *const Branch as *mut Branch) };
        BranchPtr(ptr)
    }
}

impl Into<Value> for BranchPtr {
    /// Converts current branch data into a [Value]. It uses a type ref information to resolve,
    /// which value variant is a correct one for this branch. Since branch represent only complex
    /// types [Value::Any] will never be returned from this method.
    fn into(self) -> Value {
        match self.type_ref() {
            TypeRef::Array => Value::YArray(ArrayRef::from(self)),
            TypeRef::Map => Value::YMap(MapRef::from(self)),
            TypeRef::Text => Value::YText(TextRef::from(self)),
            TypeRef::XmlElement(_) => Value::YXmlElement(XmlElementRef::from(self)),
            TypeRef::XmlFragment => Value::YXmlFragment(XmlFragmentRef::from(self)),
            TypeRef::XmlText => Value::YXmlText(XmlTextRef::from(self)),
            //TYPE_REFS_XML_HOOK => Value::YXmlHook(XmlHookRef::from(self)),
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(_) => Value::YWeakLink(WeakRef::from(self)),
            _ => Value::UndefinedRef(self),
        }
    }
}

impl Eq for BranchPtr {}

#[cfg(not(test))]
impl PartialEq for BranchPtr {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0.as_ptr(), other.0.as_ptr())
    }
}

#[cfg(test)]
impl PartialEq for BranchPtr {
    fn eq(&self, other: &Self) -> bool {
        if NonNull::eq(&self.0, &other.0) {
            true
        } else {
            let a: &Branch = self.deref();
            let b: &Branch = other.deref();
            a.eq(b)
        }
    }
}

impl std::fmt::Debug for BranchPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let branch: &Branch = &self;
        write!(f, "{}", branch)
    }
}

/// Branch describes a content of a complex Yrs data structures, such as arrays or maps.
pub struct Branch {
    /// A pointer to a first block of a indexed sequence component of this branch node. If `None`,
    /// it means that sequence is empty or a branch doesn't act as an indexed sequence. Indexed
    /// sequences include:
    ///
    /// - [Array]: all elements are stored as a double linked list, while the head of the list is
    ///   kept in this field.
    /// - [XmlElement]: this field acts as a head to a first child element stored within current XML
    ///   node.
    /// - [Text] and [XmlText]: this field point to a first chunk of text appended to collaborative
    ///   text data structure.
    pub(crate) start: Option<ItemPtr>,

    /// A map component of this branch node, used by some of the specialized complex types
    /// including:
    ///
    /// - [Map]: all of the map elements are based on this field. The value of each entry points
    ///   to the last modified value.
    /// - [XmlElement]: this field stores attributes assigned to a given XML node.
    pub(crate) map: HashMap<Arc<str>, ItemPtr>,

    /// Unique identifier of a current branch node. It can be contain either a named string - which
    /// means, this branch is a root-level complex data structure - or a block identifier. In latter
    /// case it means, that this branch is a complex type (eg. Map or Array) nested inside of
    /// another complex type.
    pub(crate) item: Option<ItemPtr>,

    pub(crate) store: Option<WeakStoreRef>,

    /// A length of an indexed sequence component of a current branch node. Map component elements
    /// are computed on demand.
    pub block_len: u64,

    pub content_len: u64,

    /// An identifier of an underlying complex data type (eg. is it an Array or a Map).
    pub(crate) type_ref: TypeRef,

    pub(crate) observers: Observer<Event>,

    pub(crate) deep_observers: Observer<Events>,
}

impl std::fmt::Debug for Branch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Eq for Branch {}

impl PartialEq for Branch {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item
            && self.start == other.start
            && self.map == other.map
            && self.block_len == other.block_len
            && self.type_ref == other.type_ref
    }
}

impl Branch {
    pub fn new(type_ref: TypeRef) -> Arc<Self> {
        Arc::new(Self {
            start: None,
            map: HashMap::default(),
            block_len: 0,
            content_len: 0,
            item: None,
            store: None,
            type_ref,
            observers: Observer::default(),
            deep_observers: Observer::default(),
        })
    }

    /// Returns an identifier of an underlying complex data type (eg. is it an Array or a Map).
    pub fn type_ref(&self) -> &TypeRef {
        &self.type_ref
    }

    pub(crate) fn repair_type_ref(&mut self, type_ref: TypeRef) {
        if self.type_ref == TypeRef::Undefined {
            self.type_ref = type_ref;
        }
    }

    /// Returns a length of an indexed sequence component of a current branch node.
    /// Map component elements are computed on demand.
    pub fn len(&self) -> u64 {
        self.block_len
    }

    pub fn content_len(&self) -> u64 {
        self.content_len
    }

    /// Get iterator over (String, Block) entries of a map component of a current root type.
    /// Deleted blocks are skipped by this iterator.
    pub(crate) fn entries<'a, T: ReadTxn + 'a>(&'a self, txn: &'a T) -> Entries<'a, &'a T, T> {
        Entries::from_ref(&self.map, txn)
    }

    /// Get iterator over Block entries of an array component of a current root type.
    /// Deleted blocks are skipped by this iterator.
    pub(crate) fn iter<'a, T: ReadTxn + 'a>(&'a self, txn: &'a T) -> Iter<'a, T> {
        Iter::new(self.start.as_ref(), txn)
    }

    /// Returns a materialized value of non-deleted entry under a given `key` of a map component
    /// of a current root type.
    pub(crate) fn get<T: ReadTxn>(&self, _txn: &T, key: &str) -> Option<Value> {
        let item = self.map.get(key)?;
        if !item.is_deleted() {
            item.content.get_last()
        } else {
            None
        }
    }

    /// Given an `index` parameter, returns an item content reference which contains that index
    /// together with an offset inside of this content, which points precisely to an `index`
    /// location within wrapping item content.
    /// If `index` was outside of the array component boundary of current branch node, `None` will
    /// be returned.
    pub(crate) fn get_at(&self, mut index: u64) -> Option<(&ItemContent, usize)> {
        let mut ptr = self.start.as_ref();
        while let Some(item) = ptr.map(ItemPtr::deref) {
            let len = item.len();
            if !item.is_deleted() && item.is_countable() {
                if index < len {
                    return Some((&item.content, index as usize));
                }

                index -= len;
            }
            ptr = item.right.as_ref();
        }

        None
    }

    /// Removes an entry under given `key` of a map component of a current root type, returning
    /// a materialized representation of value stored underneath if entry existed prior deletion.
    pub(crate) fn remove(&self, txn: &mut TransactionMut, key: &str) -> Option<Value> {
        let item = *self.map.get(key)?;
        let prev = if !item.is_deleted() {
            item.content.get_last()
        } else {
            None
        };
        txn.delete(item);
        prev
    }

    /// Returns a first non-deleted item from an array component of a current root type.
    pub(crate) fn first(&self) -> Option<&Item> {
        let mut ptr = self.start.as_ref();
        while let Some(item) = ptr.map(ItemPtr::deref) {
            if item.is_deleted() {
                ptr = item.right.as_ref();
            } else {
                return Some(item);
            }
        }

        None
    }

    /// Given an `index` and start block `ptr`, returns a pair of block pointers.
    ///
    /// If `index` happens to point inside of an existing block content, such block will be split at
    /// position of an `index`. In such case left tuple value contains end of a block pointer on
    /// a left side of an `index` and a pointer to a block directly on the right side of an `index`.
    ///
    /// If `index` point to the end of a block and no splitting is necessary, tuple will return only
    /// left side (beginning of a block), while right side will be `None`.
    ///
    /// If `index` is outside of the range of an array component of current branch node, both tuple
    /// values will be `None`.
    fn index_to_ptr(
        txn: &mut TransactionMut,
        mut ptr: Option<ItemPtr>,
        mut index: u64,
    ) -> (Option<ItemPtr>, Option<ItemPtr>) {
        let encoding = txn.store.options.offset_kind;
        while let Some(item) = ptr {
            let content_len = item.content_len(encoding);
            if !item.is_deleted() && item.is_countable() {
                if index == content_len {
                    let left = ptr;
                    let right = item.right.clone();
                    return (left, right);
                } else if index < content_len {
                    let index = if let ItemContent::String(s) = &item.content {
                        s.block_offset(index, encoding)
                    } else {
                        index
                    };
                    let right = txn.store.blocks.split_block(item, index, encoding);
                    if let Some(_) = item.moved {
                        if let Some(src) = right {
                            if let Some(&prev_dst) = txn.prev_moved.get(&item) {
                                txn.prev_moved.insert(src, prev_dst);
                            }
                        }
                    }
                    return (ptr, right);
                }
                index -= content_len;
            }
            ptr = item.right.clone();
        }
        (None, None)
    }
    /// Removes up to a `len` of countable elements from current branch sequence, starting at the
    /// given `index`. Returns number of removed elements.
    pub(crate) fn remove_at(&self, txn: &mut TransactionMut, index: u64, len: u64) -> u64 {
        let mut remaining = len;
        let start = { self.start };
        let (_, mut ptr) = if index == 0 {
            (None, start)
        } else {
            Branch::index_to_ptr(txn, start, index)
        };
        while remaining > 0 {
            if let Some(item) = ptr {
                let encoding = txn.store().options.offset_kind;
                if !item.is_deleted() {
                    let content_len = item.content_len(encoding);
                    let (l, r) = if remaining < content_len {
                        let offset = if let ItemContent::String(s) = &item.content {
                            s.block_offset(remaining, encoding)
                        } else {
                            remaining
                        };
                        remaining = 0;
                        let new_right = txn.store.blocks.split_block(item, offset, encoding);
                        if let Some(_) = item.moved {
                            if let Some(src) = new_right {
                                if let Some(&prev_dst) = txn.prev_moved.get(&item) {
                                    txn.prev_moved.insert(src, prev_dst);
                                }
                            }
                        }
                        (item, new_right)
                    } else {
                        remaining -= content_len;
                        (item, item.right.clone())
                    };
                    txn.delete(l);
                    ptr = r;
                } else {
                    ptr = item.right.clone();
                }
            } else {
                break;
            }
        }

        len - remaining
    }

    /// Inserts a preliminary `value` into a current branch indexed sequence component at the given
    /// `index`. Returns an item reference created as a result of this operation.
    pub(crate) fn insert_at<V: Prelim>(
        &self,
        txn: &mut TransactionMut,
        index: u64,
        value: V,
    ) -> ItemPtr {
        let (start, parent) = {
            if index <= self.len() {
                (self.start, BranchPtr::from(self))
            } else {
                panic!("Cannot insert item at index over the length of an array")
            }
        };
        let (left, right) = if index == 0 {
            (None, None)
        } else {
            Branch::index_to_ptr(txn, start, index)
        };
        let pos = ItemPosition {
            parent: parent.into(),
            left,
            right,
            index: 0,
            current_attrs: None,
        };

        txn.create_item(&pos, value, None)
    }

    pub(crate) fn path(from: BranchPtr, to: BranchPtr) -> Path {
        let parent = from;
        let mut child = to;
        let mut path = VecDeque::default();
        while let Some(item) = &child.item {
            if parent.item == child.item {
                break;
            }
            let item_id = item.id.clone();
            let parent_sub = item.parent_sub.clone();
            child = *item.parent.as_branch().unwrap();
            if let Some(parent_sub) = parent_sub {
                // parent is map-ish
                path.push_front(PathSegment::Key(parent_sub));
            } else {
                // parent is array-ish
                let mut i = 0;
                let mut c = child.start;
                while let Some(ptr) = c {
                    if ptr.id() == &item_id {
                        break;
                    }
                    if !ptr.is_deleted() && ptr.is_countable() {
                        i += ptr.len();
                    }
                    c = ptr.right;
                }
                path.push_front(PathSegment::Index(i));
            }
        }
        path
    }

    pub fn observe<F>(&mut self, f: F) -> Subscription
    where
        F: Fn(&TransactionMut, &Event) -> () + 'static,
    {
        self.observers.subscribe(f)
    }

    pub fn observe_deep<F>(&mut self, f: F) -> Subscription
    where
        F: Fn(&TransactionMut, &Events) -> () + 'static,
    {
        self.deep_observers.subscribe(f)
    }

    pub(crate) fn is_parent_of(&self, mut ptr: Option<ItemPtr>) -> bool {
        while let Some(i) = ptr.as_deref() {
            if let Some(parent) = i.parent.as_branch() {
                if parent.deref() == self {
                    return true;
                }
                ptr = parent.item;
            } else {
                break;
            }
        }
        false
    }

    pub(crate) fn make_event(&self, keys: HashSet<Option<Arc<str>>>) -> Option<Event> {
        let self_ptr = BranchPtr::from(self);
        let event = match self.type_ref() {
            TypeRef::Array => Event::Array(ArrayEvent::new(self_ptr)),
            TypeRef::Map => Event::Map(MapEvent::new(self_ptr, keys)),
            TypeRef::Text => Event::Text(TextEvent::new(self_ptr)),
            TypeRef::XmlElement(_) | TypeRef::XmlFragment => {
                Event::XmlFragment(XmlEvent::new(self_ptr, keys))
            }
            TypeRef::XmlText => Event::XmlText(XmlTextEvent::new(self_ptr, keys)),
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(_) => Event::Weak(WeakEvent::new(self_ptr)),
            _ => return None,
        };

        Some(event)
    }
}

/// Trait implemented by all Y-types, allowing for observing events which are emitted by
/// nested types.
pub trait DeepObservable {
    /// Subscribe a callback `f` for all events emitted by this and nested collaborative types.
    /// Callback is accepting transaction which triggered that event and event itself, wrapped
    /// within an [Event] structure.
    ///
    /// In case when a nested shared type (e.g. [MapRef],[ArrayRef],[TextRef]) is being removed,
    /// all of its contents will be removed first. So the observed value will be empty. For example,
    /// The value wrapped in the [EntryChange::Removed] of the [Event::Map] will be empty.
    ///
    /// This method returns a subscription, which will automatically unsubscribe current callback
    /// when dropped.
    fn observe_deep<F>(&mut self, f: F) -> Subscription
    where
        F: Fn(&TransactionMut, &Events) -> () + 'static;
}

impl<T> DeepObservable for T
where
    T: AsMut<Branch>,
{
    fn observe_deep<F>(&mut self, f: F) -> Subscription
    where
        F: Fn(&TransactionMut, &Events) -> () + 'static,
    {
        self.as_mut().observe_deep(f)
    }
}

/// Value that can be returned by Yrs data types. This includes [Any] which is an extension
/// representation of JSON, but also nested complex collaborative structures specific to Yrs.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Any value that it treated as a single element in it's entirety.
    Any(Any),
    /// Instance of a [TextRef].
    YText(TextRef),
    /// Instance of an [ArrayRef].
    YArray(ArrayRef),
    /// Instance of a [MapRef].
    YMap(MapRef),
    /// Instance of a [XmlElementRef].
    YXmlElement(XmlElementRef),
    /// Instance of a [XmlFragmentRef].
    YXmlFragment(XmlFragmentRef),
    /// Instance of a [XmlTextRef].
    YXmlText(XmlTextRef),
    /// Subdocument.
    YDoc(Doc),
    /// Instance of a [WeakRef] or unspecified type (requires manual casting).
    #[cfg(feature = "weak")]
    YWeakLink(WeakRef<BranchPtr>),
    /// Instance of a shared collection of undefined type. Usually happens when it refers to a root
    /// type that has not been defined locally. Can also refer to a [WeakRef] if "weak" feature flag
    /// was not set.
    UndefinedRef(BranchPtr),
}

impl Default for Value {
    fn default() -> Self {
        Value::Any(Any::Null)
    }
}

impl Value {
    #[inline]
    pub fn cast<T>(self) -> Result<T, Self>
    where
        T: TryFrom<Self, Error = Self>,
    {
        T::try_from(self)
    }

    /// Converts current value into stringified representation.
    pub fn to_string<T: ReadTxn>(self, txn: &T) -> String {
        match self {
            Value::Any(a) => a.to_string(),
            Value::YText(v) => v.get_string(txn),
            Value::YArray(v) => v.to_json(txn).to_string(),
            Value::YMap(v) => v.to_json(txn).to_string(),
            Value::YXmlElement(v) => v.get_string(txn),
            Value::YXmlFragment(v) => v.get_string(txn),
            Value::YXmlText(v) => v.get_string(txn),
            Value::YDoc(v) => v.to_string(),
            #[cfg(feature = "weak")]
            Value::YWeakLink(v) => {
                let text_ref: crate::WeakRef<TextRef> = crate::WeakRef::from(v);
                text_ref.get_string(txn)
            }
            Value::UndefinedRef(_) => "".to_string(),
        }
    }
}

impl<T> From<T> for Value
where
    T: Into<Any>,
{
    fn from(v: T) -> Self {
        let any: Any = v.into();
        Value::Any(any)
    }
}

//FIXME: what we would like to have is an automatic trait implementation of TryFrom<Value> for
// any type that implements TryFrom<Any,Error=Any>, but this causes compiler error.
macro_rules! impl_try_from {
    ($t:ty) => {
        impl TryFrom<Value> for $t {
            type Error = Value;

            fn try_from(value: Value) -> Result<Self, Self::Error> {
                match value {
                    Value::Any(any) => any.try_into().map_err(Value::Any),
                    other => Err(other),
                }
            }
        }
    };
}

impl_try_from!(bool);
impl_try_from!(f32);
impl_try_from!(f64);
impl_try_from!(i16);
impl_try_from!(i32);
impl_try_from!(i64);
impl_try_from!(u16);
impl_try_from!(u32);
impl_try_from!(u64);
impl_try_from!(isize);
impl_try_from!(String);
impl_try_from!(Arc<str>);
impl_try_from!(Vec<u8>);
impl_try_from!(Arc<[u8]>);

impl ToJson for Value {
    /// Converts current value into [Any] object equivalent that resembles enhanced JSON payload.
    /// Rules are:
    ///
    /// - Primitive types ([Value::Any]) are passed right away, as no transformation is needed.
    /// - [Value::YArray] is converted into JSON-like array.
    /// - [Value::YMap] is converted into JSON-like object map.
    /// - [Value::YText], [Value::YXmlText] and [Value::YXmlElement] are converted into strings
    ///   (XML types are stringified XML representation).
    fn to_json<T: ReadTxn>(&self, txn: &T) -> Any {
        match self {
            Value::Any(a) => a.clone(),
            Value::YText(v) => Any::from(v.get_string(txn)),
            Value::YArray(v) => v.to_json(txn),
            Value::YMap(v) => v.to_json(txn),
            Value::YXmlElement(v) => Any::from(v.get_string(txn)),
            Value::YXmlText(v) => Any::from(v.get_string(txn)),
            Value::YXmlFragment(v) => Any::from(v.get_string(txn)),
            Value::YDoc(doc) => any!({"guid": doc.guid().as_ref()}),
            #[cfg(feature = "weak")]
            Value::YWeakLink(_) => Any::Undefined,
            Value::UndefinedRef(_) => Any::Undefined,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Any(v) => std::fmt::Display::fmt(v, f),
            Value::YText(_) => write!(f, "TextRef"),
            Value::YArray(_) => write!(f, "ArrayRef"),
            Value::YMap(_) => write!(f, "MapRef"),
            Value::YXmlElement(_) => write!(f, "XmlElementRef"),
            Value::YXmlFragment(_) => write!(f, "XmlFragmentRef"),
            Value::YXmlText(_) => write!(f, "XmlTextRef"),
            #[cfg(feature = "weak")]
            Value::YWeakLink(_) => write!(f, "WeakRef"),
            Value::YDoc(v) => write!(f, "Doc(guid:{})", v.options().guid),
            Value::UndefinedRef(_) => write!(f, "UndefinedRef"),
        }
    }
}

impl std::fmt::Display for Branch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.type_ref() {
            TypeRef::Array => {
                if let Some(ptr) = self.start {
                    write!(f, "YArray(start: {})", ptr)
                } else {
                    write!(f, "YArray")
                }
            }
            TypeRef::Map => {
                write!(f, "YMap(")?;
                let mut iter = self.map.iter();
                if let Some((k, v)) = iter.next() {
                    write!(f, "'{}': {}", k, v)?;
                }
                while let Some((k, v)) = iter.next() {
                    write!(f, ", '{}': {}", k, v)?;
                }
                write!(f, ")")
            }
            TypeRef::Text => {
                if let Some(ptr) = self.start.as_ref() {
                    write!(f, "YText(start: {})", ptr)
                } else {
                    write!(f, "YText")
                }
            }
            TypeRef::XmlFragment => {
                write!(f, "YXmlFragment")?;
                if let Some(start) = self.start.as_ref() {
                    write!(f, "(start: {})", start)?;
                }
                Ok(())
            }
            TypeRef::XmlElement(name) => {
                write!(f, "YXmlElement('{}',", name)?;
                if let Some(start) = self.start.as_ref() {
                    write!(f, "(start: {})", start)?;
                }
                if !self.map.is_empty() {
                    write!(f, " {{")?;
                    let mut iter = self.map.iter();
                    if let Some((k, v)) = iter.next() {
                        write!(f, "'{}': {}", k, v)?;
                    }
                    while let Some((k, v)) = iter.next() {
                        write!(f, ", '{}': {}", k, v)?;
                    }
                    write!(f, "}}")?;
                }
                Ok(())
            }
            TypeRef::XmlHook => {
                write!(f, "YXmlHook(")?;
                let mut iter = self.map.iter();
                if let Some((k, v)) = iter.next() {
                    write!(f, "'{}': {}", k, v)?;
                }
                while let Some((k, v)) = iter.next() {
                    write!(f, ", '{}': {}", k, v)?;
                }
                write!(f, ")")
            }
            TypeRef::XmlText => {
                if let Some(ptr) = self.start {
                    write!(f, "YXmlText(start: {})", ptr)
                } else {
                    write!(f, "YXmlText")
                }
            }
            TypeRef::SubDoc => {
                write!(f, "Subdoc")
            }
            #[cfg(feature = "weak")]
            TypeRef::WeakLink(w) => {
                if w.is_single() {
                    write!(f, "WeakRef({})", w.quote_start)
                } else {
                    write!(f, "WeakRef({}..{})", w.quote_start, w.quote_end)
                }
            }
            TypeRef::Undefined => {
                write!(f, "UnknownRef")?;
                if let Some(start) = self.start.as_ref() {
                    write!(f, "(start: {})", start)?;
                }
                if !self.map.is_empty() {
                    write!(f, " {{")?;
                    let mut iter = self.map.iter();
                    if let Some((k, v)) = iter.next() {
                        write!(f, "'{}': {}", k, v)?;
                    }
                    while let Some((k, v)) = iter.next() {
                        write!(f, ", '{}': {}", k, v)?;
                    }
                    write!(f, "}}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Entries<'a, B, T> {
    iter: std::collections::hash_map::Iter<'a, Arc<str>, ItemPtr>,
    txn: B,
    _marker: PhantomData<T>,
}

impl<'a, B, T: ReadTxn> Entries<'a, B, T>
where
    B: Borrow<T>,
    T: ReadTxn,
{
    pub fn new(source: &'a HashMap<Arc<str>, ItemPtr>, txn: B) -> Self {
        Entries {
            iter: source.iter(),
            txn,
            _marker: PhantomData::default(),
        }
    }
}

impl<'a, T: ReadTxn> Entries<'a, T, T>
where
    T: Borrow<T> + ReadTxn,
{
    pub fn from(source: &'a HashMap<Arc<str>, ItemPtr>, txn: T) -> Self {
        Entries::new(source, txn)
    }
}

impl<'a, T: ReadTxn> Entries<'a, &'a T, T>
where
    T: Borrow<T> + ReadTxn,
{
    pub fn from_ref(source: &'a HashMap<Arc<str>, ItemPtr>, txn: &'a T) -> Self {
        Entries::new(source, txn)
    }
}

impl<'a, B, T> Iterator for Entries<'a, B, T>
where
    B: Borrow<T>,
    T: ReadTxn,
{
    type Item = (&'a str, &'a Item);

    fn next(&mut self) -> Option<Self::Item> {
        let (mut key, mut ptr) = self.iter.next()?;
        while ptr.is_deleted() {
            (key, ptr) = self.iter.next()?;
        }
        Some((key, ptr))
    }
}

pub(crate) struct Iter<'a, T> {
    ptr: Option<&'a ItemPtr>,
    _txn: &'a T,
}

impl<'a, T: ReadTxn> Iter<'a, T> {
    fn new(ptr: Option<&'a ItemPtr>, txn: &'a T) -> Self {
        Iter { ptr, _txn: txn }
    }
}

impl<'a, T: ReadTxn> Iterator for Iter<'a, T> {
    type Item = &'a Item;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.ptr.take()?;
        self.ptr = item.right.as_ref();
        Some(item)
    }
}

/// Type pointer - used to localize a complex [Branch] node within a scope of a document store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TypePtr {
    /// Temporary value - used only when block is deserialized right away, but had not been
    /// integrated into block store yet. As part of block integration process, items are
    /// repaired and their fields (including parent) are being rewired.
    Unknown,

    /// Pointer to another block. Used in nested data types ie. YMap containing another YMap.
    Branch(BranchPtr),

    /// Temporary state representing top-level type.
    Named(Arc<str>),

    /// Temporary state representing nested-level type.
    ID(ID),
}

impl TypePtr {
    pub(crate) fn as_branch(&self) -> Option<&BranchPtr> {
        if let TypePtr::Branch(ptr) = self {
            Some(ptr)
        } else {
            None
        }
    }
}

impl std::fmt::Display for TypePtr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TypePtr::Unknown => write!(f, "unknown"),
            TypePtr::Branch(ptr) => {
                if let Some(i) = ptr.item {
                    write!(f, "{}", i.id())
                } else {
                    write!(f, "null")
                }
            }
            TypePtr::ID(id) => write!(f, "{}", id),
            TypePtr::Named(name) => write!(f, "{}", name),
        }
    }
}

/// A path describing nesting structure between shared collections containing each other. It's a
/// collection of segments which refer to either index (in case of [Array] or [XmlElement]) or
/// string key (in case of [Map]) where successor shared collection can be found within subsequent
/// parent types.
pub type Path = VecDeque<PathSegment>;

/// A single segment of a [Path].
#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    /// Key segments are used to inform how to access child shared collections within a [Map] types.
    Key(Arc<str>),

    /// Index segments are used to inform how to access child shared collections within an [Array]
    /// or [XmlElement] types.
    Index(u64),
}

pub(crate) struct ChangeSet<D> {
    added: HashSet<ID>,
    deleted: HashSet<ID>,
    delta: Vec<D>,
}

impl<D> ChangeSet<D> {
    pub fn new(added: HashSet<ID>, deleted: HashSet<ID>, delta: Vec<D>) -> Self {
        ChangeSet {
            added,
            deleted,
            delta,
        }
    }
}

/// A single change done over an array-component of shared data type.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Determines a change that resulted in adding a consecutive number of new elements:
    /// - For [Array] it's a range of inserted elements.
    /// - For [XmlElement] it's a range of inserted child XML nodes.
    Added(Vec<Value>),

    /// Determines a change that resulted in removing a consecutive range of existing elements,
    /// either XML child nodes for [XmlElement] or various elements stored in an [Array].
    Removed(u64),

    /// Determines a number of consecutive unchanged elements. Used to recognize non-edited spaces
    /// between [Change::Added] and/or [Change::Removed] chunks.
    Retain(u64),
}

/// A single change done over a map-component of shared data type.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryChange {
    /// Informs about a new value inserted under specified entry.
    Inserted(Value),

    /// Informs about a change of old value (1st field) to a new one (2nd field) under
    /// a corresponding entry.
    Updated(Value, Value),

    /// Informs about a removal of a corresponding entry - contains a removed value.
    Removed(Value),
}

/// A single change done over a text-like types: [Text] or [XmlText].
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    /// Determines a change that resulted in insertion of a piece of text, which optionally could
    /// have been formatted with provided set of attributes.
    Inserted(Value, Option<Box<Attrs>>),

    /// Determines a change that resulted in removing a consecutive range of characters.
    Deleted(u64),

    /// Determines a number of consecutive unchanged characters. Used to recognize non-edited spaces
    /// between [Delta::Inserted] and/or [Delta::Deleted] chunks. Can contain an optional set of
    /// attributes, which have been used to format an existing piece of text.
    Retain(u64, Option<Box<Attrs>>),
}

/// An alias for map of attributes used as formatting parameters by [Text] and [XmlText] types.
pub type Attrs = HashMap<Arc<str>, Any>;

pub(crate) fn event_keys(
    txn: &TransactionMut,
    target: BranchPtr,
    keys_changed: &HashSet<Option<Arc<str>>>,
) -> HashMap<Arc<str>, EntryChange> {
    let mut keys = HashMap::new();
    for opt in keys_changed.iter() {
        if let Some(key) = opt {
            let block = target.map.get(key.as_ref()).cloned();
            if let Some(item) = block.as_deref() {
                if item.id.clock >= txn.before_state.get(&item.id.client) {
                    let mut prev = item.left;
                    while let Some(p) = prev.as_deref() {
                        if !txn.has_added(&p.id) {
                            break;
                        }
                        prev = p.left;
                    }

                    if txn.has_deleted(&item.id) {
                        if let Some(prev) = prev.as_deref() {
                            if txn.has_deleted(&prev.id) {
                                let old_value = prev.content.get_last().unwrap_or_default();
                                keys.insert(key.clone(), EntryChange::Removed(old_value));
                            }
                        }
                    } else {
                        let new_value = item.content.get_last().unwrap();
                        if let Some(prev) = prev.as_deref() {
                            if txn.has_deleted(&prev.id) {
                                let old_value = prev.content.get_last().unwrap_or_default();
                                keys.insert(
                                    key.clone(),
                                    EntryChange::Updated(old_value, new_value),
                                );

                                continue;
                            }
                        }

                        keys.insert(key.clone(), EntryChange::Inserted(new_value));
                    }
                } else if txn.has_deleted(&item.id) {
                    let old_value = item.content.get_last().unwrap_or_default();
                    keys.insert(key.clone(), EntryChange::Removed(old_value));
                }
            }
        }
    }

    keys
}

pub(crate) fn event_change_set(txn: &TransactionMut, start: Option<ItemPtr>) -> ChangeSet<Change> {
    let mut added = HashSet::new();
    let mut deleted = HashSet::new();
    let mut delta = Vec::new();

    let mut moved_stack = Vec::new();
    let mut curr_move: Option<ItemPtr> = None;
    let mut curr_move_is_new = false;
    let mut curr_move_is_deleted = false;
    let mut curr_move_end: Option<ItemPtr> = None;
    let mut last_op = None;

    #[derive(Default)]
    struct MoveStackItem {
        end: Option<ItemPtr>,
        moved: Option<ItemPtr>,
        is_new: bool,
        is_deleted: bool,
    }

    fn is_moved_by_new(ptr: Option<ItemPtr>, txn: &TransactionMut) -> bool {
        let mut moved = ptr;
        while let Some(item) = moved.as_deref() {
            if txn.has_added(&item.id) {
                return true;
            } else {
                moved = item.moved;
            }
        }

        false
    }

    let encoding = txn.store().options.offset_kind;
    let mut current = start;
    loop {
        if current == curr_move_end && curr_move.is_some() {
            current = curr_move;
            let item: MoveStackItem = moved_stack.pop().unwrap_or_default();
            curr_move_is_new = item.is_new;
            curr_move_is_deleted = item.is_deleted;
            curr_move = item.moved;
            curr_move_end = item.end;
        } else {
            if let Some(item) = current {
                if let ItemContent::Move(m) = &item.content {
                    if item.moved == curr_move {
                        moved_stack.push(MoveStackItem {
                            end: curr_move_end,
                            moved: curr_move,
                            is_new: curr_move_is_new,
                            is_deleted: curr_move_is_deleted,
                        });
                        let txn = unsafe {
                            //TODO: remove this - find a way to work with get_moved_coords
                            // without need for &mut Transaction
                            (txn as *const TransactionMut as *mut TransactionMut)
                                .as_mut()
                                .unwrap()
                        };
                        let (start, end) = m.get_moved_coords(txn);
                        curr_move = current;
                        curr_move_end = end;
                        curr_move_is_new = curr_move_is_new || txn.has_added(&item.id);
                        curr_move_is_deleted = curr_move_is_deleted || item.is_deleted();
                        current = start;
                        continue; // do not move to item.right
                    }
                } else if item.moved != curr_move {
                    if !curr_move_is_new
                        && item.is_countable()
                        && (!item.is_deleted() || txn.has_deleted(&item.id))
                        && !txn.has_added(&item.id)
                        && (item.moved.is_none()
                            || curr_move_is_deleted
                            || is_moved_by_new(item.moved, txn))
                        && (txn.prev_moved.get(&item).cloned() == curr_move)
                    {
                        match item.moved {
                            Some(ptr) if txn.has_added(ptr.id()) => {
                                let len = item.content_len(encoding);
                                last_op = match last_op.take() {
                                    Some(Change::Removed(i)) => Some(Change::Removed(i + len)),
                                    Some(op) => {
                                        delta.push(op);
                                        Some(Change::Removed(len))
                                    }
                                    None => Some(Change::Removed(len)),
                                };
                            }
                            _ => {}
                        }
                    }
                } else if item.is_deleted() {
                    if !curr_move_is_new
                        && txn.has_deleted(&item.id)
                        && !txn.has_added(&item.id)
                        && !txn.prev_moved.contains_key(&item)
                    {
                        let removed = match last_op.take() {
                            None => 0,
                            Some(Change::Removed(c)) => c,
                            Some(other) => {
                                delta.push(other);
                                0
                            }
                        };
                        last_op = Some(Change::Removed(removed + item.len()));
                        deleted.insert(item.id);
                    } // else nop
                } else {
                    if curr_move_is_new
                        || txn.has_added(&item.id)
                        || txn.prev_moved.contains_key(&item)
                    {
                        let mut inserts = match last_op.take() {
                            None => Vec::with_capacity(item.len() as usize),
                            Some(Change::Added(values)) => values,
                            Some(other) => {
                                delta.push(other);
                                Vec::with_capacity(item.len() as usize)
                            }
                        };
                        inserts.append(&mut item.content.get_content());
                        last_op = Some(Change::Added(inserts));
                        added.insert(item.id);
                    } else {
                        let retain = match last_op.take() {
                            None => 0,
                            Some(Change::Retain(c)) => c,
                            Some(other) => {
                                delta.push(other);
                                0
                            }
                        };
                        last_op = Some(Change::Retain(retain + item.len()));
                    }
                }
            } else {
                break;
            }
        }

        current = if let Some(i) = current.as_deref() {
            i.right
        } else {
            None
        };
    }

    match last_op.take() {
        None | Some(Change::Retain(_)) => { /* do nothing */ }
        Some(change) => delta.push(change),
    }

    ChangeSet::new(added, deleted, delta)
}

pub struct Events(Vec<NonNull<Event>>);

impl Events {
    pub(crate) fn new(events: &mut Vec<&Event>) -> Self {
        events.sort_by(|&a, &b| {
            let path1 = a.path();
            let path2 = b.path();
            path1.len().cmp(&path2.len())
        });
        let mut inner = Vec::with_capacity(events.len());
        for &e in events.iter() {
            inner.push(unsafe { NonNull::new_unchecked(e as *const Event as *mut Event) });
        }
        Events(inner)
    }

    pub fn iter(&self) -> EventsIter {
        EventsIter(self.0.iter())
    }
}

pub struct EventsIter<'a>(std::slice::Iter<'a, NonNull<Event>>);

impl<'a> Iterator for EventsIter<'a> {
    type Item = &'a Event;

    fn next(&mut self) -> Option<Self::Item> {
        let e = self.0.next()?;
        Some(unsafe { e.as_ref() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a> ExactSizeIterator for EventsIter<'a> {
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Generalized wrapper around events fired by specialized shared data types.
pub enum Event {
    Text(TextEvent),
    Array(ArrayEvent),
    Map(MapEvent),
    XmlFragment(XmlEvent),
    XmlText(XmlTextEvent),
    #[cfg(feature = "weak")]
    Weak(WeakEvent),
}

impl AsRef<TextEvent> for Event {
    fn as_ref(&self) -> &TextEvent {
        if let Event::Text(e) = self {
            e
        } else {
            panic!("subscribed callback expected TextRef collection");
        }
    }
}

impl AsRef<ArrayEvent> for Event {
    fn as_ref(&self) -> &ArrayEvent {
        if let Event::Array(e) = self {
            e
        } else {
            panic!("subscribed callback expected ArrayRef collection");
        }
    }
}

impl AsRef<MapEvent> for Event {
    fn as_ref(&self) -> &MapEvent {
        if let Event::Map(e) = self {
            e
        } else {
            panic!("subscribed callback expected MapRef collection");
        }
    }
}

impl AsRef<XmlTextEvent> for Event {
    fn as_ref(&self) -> &XmlTextEvent {
        if let Event::XmlText(e) = self {
            e
        } else {
            panic!("subscribed callback expected XmlTextRef collection");
        }
    }
}

impl AsRef<XmlEvent> for Event {
    fn as_ref(&self) -> &XmlEvent {
        if let Event::XmlFragment(e) = self {
            e
        } else {
            panic!("subscribed callback expected Xml node");
        }
    }
}

#[cfg(feature = "weak")]
impl AsRef<WeakEvent> for Event {
    fn as_ref(&self) -> &WeakEvent {
        if let Event::Weak(e) = self {
            e
        } else {
            panic!("subscribed callback expected WeakRef reference");
        }
    }
}

impl Event {
    pub(crate) fn set_current_target(&mut self, target: BranchPtr) {
        match self {
            Event::Text(e) => e.current_target = target,
            Event::Array(e) => e.current_target = target,
            Event::Map(e) => e.current_target = target,
            Event::XmlText(e) => e.current_target = target,
            Event::XmlFragment(e) => e.current_target = target,
            #[cfg(feature = "weak")]
            Event::Weak(e) => e.current_target = target,
        }
    }

    /// Returns a path from root type to a shared type which triggered current [Event]. This path
    /// consists of string names or indexes, which can be used to access nested type.
    pub fn path(&self) -> Path {
        match self {
            Event::Text(e) => e.path(),
            Event::Array(e) => e.path(),
            Event::Map(e) => e.path(),
            Event::XmlText(e) => e.path(),
            Event::XmlFragment(e) => e.path(),
            #[cfg(feature = "weak")]
            Event::Weak(e) => e.path(),
        }
    }

    /// Returns a shared data types which triggered current [Event].
    pub fn target(&self) -> Value {
        match self {
            Event::Text(e) => Value::YText(e.target().clone()),
            Event::Array(e) => Value::YArray(e.target().clone()),
            Event::Map(e) => Value::YMap(e.target().clone()),
            Event::XmlText(e) => Value::YXmlText(e.target().clone()),
            Event::XmlFragment(e) => match e.target() {
                XmlNode::Element(n) => Value::YXmlElement(n.clone()),
                XmlNode::Fragment(n) => Value::YXmlFragment(n.clone()),
                XmlNode::Text(n) => Value::YXmlText(n.clone()),
            },
            #[cfg(feature = "weak")]
            Event::Weak(e) => Value::YWeakLink(e.as_target().clone()),
        }
    }
}

pub trait ToJson {
    /// Converts all contents of a current type into a JSON-like representation.
    fn to_json<T: ReadTxn>(&self, txn: &T) -> Any;
}
