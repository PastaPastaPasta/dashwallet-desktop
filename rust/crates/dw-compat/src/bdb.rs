//! Read-only Berkeley DB 4.8 btree reader for legacy `wallet.dat`.
//!
//! The page layout follows Bitcoin Core's `BerkeleyRODatabase`
//! (`src/wallet/migrate.cpp`): an outer btree whose leaf names the `main`
//! subdatabase, then that subdatabase's meta page and its btree of
//! key/value records. Pages are bounds-checked, allocations are capped, and
//! a repeated page number is a cycle, not a loop.

use crate::walletdat::{WalletDatError, corrupt};

const PAGE_MIN: usize = 512;
const PAGE_MAX: usize = 65_536;
/// Largest key or value copied out of a file. Wallet records we keep are
/// far smaller; this is the same cap as a Core serialized field.
const MAX_RECORD: usize = 32 * 1024 * 1024;
const MAX_RECORDS: usize = 500_000;
pub(crate) const MAX_FILE: usize = 256 * 1024 * 1024;
const BTREE_MAGIC: u32 = 0x0005_3162;
const BTREE_VERSION: u32 = 9;
const FLAG_SUBDB: u32 = 0x20;
const PAGE_HEADER: usize = 26;
pub(crate) const TYPE_INTERNAL: u8 = 3;
pub(crate) const TYPE_LEAF: u8 = 5;
pub(crate) const TYPE_OVERFLOW: u8 = 7;
const TYPE_META: u8 = 9;
const REC_KEYDATA: u8 = 1;
const REC_OVERFLOW: u8 = 3;
const REC_DELETE: u8 = 0x80;

struct Meta {
    other_endian: bool,
    pagesize: usize,
    last_page: u32,
    root: u32,
}

enum Item {
    Data { deleted: bool, data: Vec<u8> },
    Overflow { deleted: bool, page: u32, len: u32 },
    Child { deleted: bool, page: u32 },
}

fn u32_at(page: &[u8], off: usize, swap: bool) -> Result<u32, WalletDatError> {
    let b: [u8; 4] = page
        .get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| corrupt("truncated page"))?;
    let v = u32::from_le_bytes(b);
    Ok(if swap { v.swap_bytes() } else { v })
}

fn u16_at(page: &[u8], off: usize, swap: bool) -> Result<u16, WalletDatError> {
    let b: [u8; 2] = page
        .get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| corrupt("truncated page"))?;
    let v = u16::from_le_bytes(b);
    Ok(if swap { v.swap_bytes() } else { v })
}

fn read_meta(page: &[u8], expected: u32) -> Result<Meta, WalletDatError> {
    if page.len() < 92 {
        return Err(corrupt("truncated meta page"));
    }
    let magic = u32::from_le_bytes(
        page[12..16]
            .try_into()
            .map_err(|_| corrupt("truncated meta page"))?,
    );
    let other_endian = match magic {
        BTREE_MAGIC => false,
        x if x == BTREE_MAGIC.swap_bytes() => true,
        _ => return Err(corrupt("not a Berkeley DB file")),
    };
    let page_num = u32_at(page, 8, other_endian)?;
    let version = u32_at(page, 16, other_endian)?;
    let pagesize = u32_at(page, 20, other_endian)? as usize;
    let encrypt = *page.get(24).ok_or_else(|| corrupt("truncated meta page"))?;
    let kind = *page.get(25).ok_or_else(|| corrupt("truncated meta page"))?;
    let last_page = u32_at(page, 32, other_endian)?;
    let flags = u32_at(page, 48, other_endian)?;
    let root = u32_at(page, 88, other_endian)?;
    if page_num != expected {
        return Err(corrupt("meta page number mismatch"));
    }
    if version != BTREE_VERSION {
        return Err(corrupt("unsupported Berkeley DB version"));
    }
    if !(PAGE_MIN..=PAGE_MAX).contains(&pagesize) || !pagesize.is_power_of_two() {
        return Err(corrupt("bad page size"));
    }
    if kind != TYPE_META {
        return Err(corrupt("not a btree database"));
    }
    if flags != FLAG_SUBDB {
        return Err(corrupt("unexpected database flags"));
    }
    if encrypt != 0 {
        return Err(corrupt("Berkeley DB encryption is not supported"));
    }
    Ok(Meta {
        other_endian,
        pagesize,
        last_page,
        root,
    })
}

fn page(file: &[u8], pagesize: usize, n: u32) -> Result<&[u8], WalletDatError> {
    let start = (n as usize)
        .checked_mul(pagesize)
        .ok_or_else(|| corrupt("page number overflow"))?;
    let end = start
        .checked_add(pagesize)
        .ok_or_else(|| corrupt("page number overflow"))?;
    file.get(start..end)
        .ok_or_else(|| corrupt("page past end of file"))
}

fn lsn_is_reset(page: &[u8], swap: bool) -> Result<(), WalletDatError> {
    let file = u32_at(page, 0, swap)?;
    let off = u32_at(page, 4, swap)?;
    if file == 0 && off == 1 {
        Ok(())
    } else {
        Err(corrupt(
            "wallet file is not flushed; close Dash Core and import again",
        ))
    }
}

struct PageHead {
    page_num: u32,
    next_page: u32,
    entries: u16,
    hf_offset: u16,
    level: u8,
    kind: u8,
}

fn read_head(page: &[u8], expected: u32, swap: bool) -> Result<PageHead, WalletDatError> {
    let head = PageHead {
        page_num: u32_at(page, 8, swap)?,
        next_page: u32_at(page, 16, swap)?,
        entries: u16_at(page, 20, swap)?,
        hf_offset: u16_at(page, 22, swap)?,
        level: *page.get(24).ok_or_else(|| corrupt("truncated page"))?,
        kind: *page.get(25).ok_or_else(|| corrupt("truncated page"))?,
    };
    if head.page_num != expected {
        return Err(corrupt("page number mismatch"));
    }
    let overflow = head.kind == TYPE_OVERFLOW;
    if overflow && head.level != 0 || !overflow && head.level < 1 {
        return Err(corrupt("bad btree level"));
    }
    Ok(head)
}

fn read_items(
    page: &[u8],
    head: &PageHead,
    swap: bool,
    internal: bool,
) -> Result<Vec<Item>, WalletDatError> {
    let entries = head.entries as usize;
    let index_end = PAGE_HEADER
        .checked_add(
            entries
                .checked_mul(2)
                .ok_or_else(|| corrupt("page entries"))?,
        )
        .ok_or_else(|| corrupt("page entries"))?;
    if index_end > page.len() {
        return Err(corrupt("page entries do not fit"));
    }
    let mut items = Vec::with_capacity(entries);
    for i in 0..entries {
        let index = u16_at(page, PAGE_HEADER + i * 2, swap)? as usize;
        // The record starts after this index slot, as Core requires.
        if index < PAGE_HEADER + (i + 1) * 2 || index >= page.len() {
            return Err(corrupt("record position not in page"));
        }
        let len = u16_at(page, index, swap)? as usize;
        let type_byte = *page
            .get(index + 2)
            .ok_or_else(|| corrupt("truncated record"))?;
        let deleted = type_byte & REC_DELETE != 0;
        let kind = type_byte & !REC_DELETE;
        let body = index + 3;
        if internal {
            if kind != REC_KEYDATA {
                return Err(corrupt("unknown record type in internal page"));
            }
            let key_end = body
                .checked_add(9)
                .and_then(|p| p.checked_add(len))
                .ok_or_else(|| corrupt("record extends past page"))?;
            if key_end > page.len() {
                return Err(corrupt("record extends past page"));
            }
            items.push(Item::Child {
                deleted,
                page: u32_at(page, body + 1, swap)?,
            });
        } else {
            match kind {
                REC_KEYDATA => {
                    let end = body
                        .checked_add(len)
                        .ok_or_else(|| corrupt("record extends past page"))?;
                    if end > page.len() {
                        return Err(corrupt("record extends past page"));
                    }
                    items.push(Item::Data {
                        deleted,
                        data: page[body..end].to_vec(),
                    });
                }
                REC_OVERFLOW => {
                    if body + 9 > page.len() {
                        return Err(corrupt("truncated overflow record"));
                    }
                    items.push(Item::Overflow {
                        deleted,
                        page: u32_at(page, body + 1, swap)?,
                        len: u32_at(page, body + 5, swap)?,
                    });
                }
                _ => return Err(corrupt("unknown record type")),
            }
        }
    }
    Ok(items)
}

fn overflow_bytes(
    file: &[u8],
    meta: &Meta,
    first: u32,
    item_len: u32,
) -> Result<Vec<u8>, WalletDatError> {
    let want = item_len as usize;
    if want > MAX_RECORD || want > file.len() {
        return Err(corrupt("overflow record is too large"));
    }
    let mut out = Vec::with_capacity(want);
    let mut next = first;
    let mut seen = vec![false; meta.last_page as usize + 1];
    while next != 0 {
        if out.len() == want {
            break;
        }
        if next > meta.last_page || seen[next as usize] {
            return Err(corrupt(if next > meta.last_page {
                "overflow page past end of file"
            } else {
                "overflow page cycle"
            }));
        }
        seen[next as usize] = true;
        let page = page(file, meta.pagesize, next)?;
        lsn_is_reset(page, meta.other_endian)?;
        let head = read_head(page, next, meta.other_endian)?;
        if head.kind != TYPE_OVERFLOW {
            return Err(corrupt("bad overflow record page type"));
        }
        let n = head.hf_offset as usize;
        if PAGE_HEADER + n > page.len() {
            return Err(corrupt("overflow data extends past page"));
        }
        let room = want - out.len();
        if n > room {
            return Err(corrupt("overflow page longer than its record"));
        }
        out.extend_from_slice(&page[PAGE_HEADER..PAGE_HEADER + n]);
        next = head.next_page;
    }
    if out.len() != want {
        return Err(corrupt("overflow record is short"));
    }
    Ok(out)
}

fn item_bytes(file: &[u8], meta: &Meta, item: &Item) -> Result<Option<Vec<u8>>, WalletDatError> {
    match item {
        Item::Data { deleted: true, .. } | Item::Overflow { deleted: true, .. } => Ok(None),
        Item::Data { data, .. } => {
            if data.len() > MAX_RECORD {
                return Err(corrupt("record is too large"));
            }
            Ok(Some(data.clone()))
        }
        Item::Overflow { page, len, .. } => Ok(Some(overflow_bytes(file, meta, *page, *len)?)),
        Item::Child { .. } => Err(corrupt("internal record on a leaf page")),
    }
}

pub(crate) type Record = (Vec<u8>, Vec<u8>);

/// Key/value pairs of the `main` subdatabase, in btree order.
pub(crate) fn read_pairs(file: &[u8]) -> Result<Vec<Record>, WalletDatError> {
    if file.len() > MAX_FILE {
        return Err(WalletDatError::Unreadable(format!(
            "wallet file is over {MAX_FILE} bytes"
        )));
    }
    if file.len() < PAGE_MIN {
        return Err(corrupt("not a Berkeley DB file"));
    }
    let outer = read_meta(file, 0)?;
    if outer.pagesize > file.len() {
        return Err(corrupt("bad page size"));
    }
    let pages = file.len() / outer.pagesize;
    if pages == 0 || outer.last_page as usize != pages - 1 {
        return Err(corrupt("last page number does not match the file"));
    }
    // Core checks every page except the last. A bad LSN means Dash Core did
    // not close the file, so the pages may be torn.
    for n in 0..outer.last_page {
        lsn_is_reset(page(file, outer.pagesize, n)?, outer.other_endian)?;
    }
    if outer.root > outer.last_page {
        return Err(corrupt("root page past end of file"));
    }
    let root_bytes = page(file, outer.pagesize, outer.root)?;
    let root_head = read_head(root_bytes, outer.root, outer.other_endian)?;
    if root_head.kind != TYPE_LEAF {
        return Err(corrupt("unexpected outer database root page type"));
    }
    if root_head.entries != 2 {
        return Err(corrupt(
            "unexpected number of entries in outer database root page",
        ));
    }
    let outer_items = read_items(root_bytes, &root_head, outer.other_endian, false)?;
    let (
        Item::Data {
            deleted: false,
            data: name,
        },
        Item::Data {
            deleted: false,
            data: loc,
        },
    ) = (&outer_items[0], &outer_items[1])
    else {
        return Err(corrupt("subdatabase record is not inline"));
    };
    if name != b"main" {
        return Err(corrupt("subdatabase has an unexpected name"));
    }
    if loc.len() != 4 {
        return Err(corrupt("subdatabase page number has unexpected length"));
    }
    let main_page = u32::from_be_bytes(
        loc.as_slice()
            .try_into()
            .map_err(|_| corrupt("subdatabase page number has unexpected length"))?,
    );
    if main_page > outer.last_page {
        return Err(corrupt("subdatabase page past end of file"));
    }
    let inner = read_meta(page(file, outer.pagesize, main_page)?, main_page)?;
    if inner.pagesize != outer.pagesize || inner.other_endian != outer.other_endian {
        return Err(corrupt("unexpected subdatabase page size"));
    }
    if inner.last_page > outer.last_page {
        return Err(corrupt("subdatabase last page past end of file"));
    }
    if inner.root > outer.last_page {
        return Err(corrupt("subdatabase root past end of file"));
    }

    let mut records = Vec::new();
    let mut stored = 0usize;
    let mut stack = vec![inner.root];
    let mut seen = vec![false; outer.last_page as usize + 1];
    while let Some(n) = stack.pop() {
        if n > outer.last_page || seen[n as usize] {
            return Err(corrupt("btree page cycle"));
        }
        seen[n as usize] = true;
        let bytes = page(file, inner.pagesize, n)?;
        let head = read_head(bytes, n, inner.other_endian)?;
        match head.kind {
            TYPE_INTERNAL => {
                let items = read_items(bytes, &head, inner.other_endian, true)?;
                for item in items.into_iter().rev() {
                    if let Item::Child {
                        deleted,
                        page: child,
                    } = item
                    {
                        if deleted {
                            continue;
                        }
                        if child > outer.last_page {
                            return Err(corrupt("child page past end of file"));
                        }
                        stack.push(child);
                    }
                }
            }
            TYPE_LEAF => {
                let items = read_items(bytes, &head, inner.other_endian, false)?;
                if items.len() % 2 != 0 {
                    return Err(corrupt("records page has an odd number of records"));
                }
                let mut i = 0;
                while i < items.len() {
                    let key_item = &items[i];
                    let val_item = &items[i + 1];
                    i += 2;
                    let (Some(key), Some(value)) = (
                        item_bytes(file, &inner, key_item)?,
                        item_bytes(file, &inner, val_item)?,
                    ) else {
                        continue;
                    };
                    stored = stored.saturating_add(key.len()).saturating_add(value.len());
                    if stored > file.len() || records.len() >= MAX_RECORDS {
                        return Err(corrupt("too many wallet records"));
                    }
                    records.push((key, value));
                }
            }
            _ => return Err(corrupt("unexpected page type")),
        }
    }
    Ok(records)
}

#[cfg(test)]
pub(crate) mod build {
    const PAGE: usize = 4096;

    fn put_u32(page: &mut [u8], off: usize, v: u32, swap: bool) {
        let v = if swap { v.swap_bytes() } else { v };
        page[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_u16(page: &mut [u8], off: usize, v: u16, swap: bool) {
        let v = if swap { v.swap_bytes() } else { v };
        page[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn data_item(payload: &[u8], deleted: bool, swap: bool) -> Vec<u8> {
        let mut v = Vec::with_capacity(3 + payload.len());
        let len = payload.len() as u16;
        let len = if swap { len.swap_bytes() } else { len };
        v.extend_from_slice(&len.to_le_bytes());
        v.push(if deleted { 1 | 0x80 } else { 1 });
        v.extend_from_slice(payload);
        v
    }

    pub(crate) fn overflow_item(page_no: u32, item_len: u32, swap: bool) -> Vec<u8> {
        let mut v = Vec::with_capacity(12);
        v.extend_from_slice(&0u16.to_le_bytes());
        v.push(3);
        v.push(0);
        let page_no = if swap { page_no.swap_bytes() } else { page_no };
        let item_len = if swap {
            item_len.swap_bytes()
        } else {
            item_len
        };
        v.extend_from_slice(&page_no.to_le_bytes());
        v.extend_from_slice(&item_len.to_le_bytes());
        v
    }

    pub(crate) fn child_item(child: u32, swap: bool) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0];
        let child = if swap { child.swap_bytes() } else { child };
        v.extend_from_slice(&child.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    struct DataHead {
        page_num: u32,
        entries: u16,
        hf: u16,
        level: u8,
        kind: u8,
        next: u32,
    }

    fn data_header(page: &mut [u8], head: DataHead, swap: bool) {
        put_u32(page, 0, 0, swap);
        put_u32(page, 4, 1, swap);
        put_u32(page, 8, head.page_num, swap);
        put_u32(page, 16, head.next, swap);
        put_u16(page, 20, head.entries, swap);
        put_u16(page, 22, head.hf, swap);
        page[24] = head.level;
        page[25] = head.kind;
    }

    pub(crate) fn pack(
        page_num: u32,
        items: &[Vec<u8>],
        kind: u8,
        level: u8,
        next: u32,
        swap: bool,
    ) -> Vec<u8> {
        let mut page = vec![0u8; PAGE];
        let mut end = PAGE;
        let mut offsets = Vec::with_capacity(items.len());
        for item in items.iter().rev() {
            assert!(
                end >= 26 + items.len() * 2 + item.len(),
                "records do not fit"
            );
            end -= item.len();
            page[end..end + item.len()].copy_from_slice(item);
            offsets.push(end as u16);
        }
        offsets.reverse();
        data_header(
            &mut page,
            DataHead {
                page_num,
                entries: items.len() as u16,
                hf: end as u16,
                level,
                kind,
                next,
            },
            swap,
        );
        for (i, off) in offsets.iter().enumerate() {
            put_u16(&mut page, 26 + i * 2, *off, swap);
        }
        page
    }

    pub(crate) fn meta(page_num: u32, root: u32, last: u32, swap: bool) -> Vec<u8> {
        let mut page = vec![0u8; PAGE];
        put_u32(&mut page, 0, 0, swap);
        put_u32(&mut page, 4, 1, swap);
        put_u32(&mut page, 8, page_num, swap);
        put_u32(&mut page, 12, super::BTREE_MAGIC, swap);
        put_u32(&mut page, 16, super::BTREE_VERSION, swap);
        put_u32(&mut page, 20, PAGE as u32, swap);
        page[25] = super::TYPE_META;
        put_u32(&mut page, 32, last, swap);
        put_u32(&mut page, 48, super::FLAG_SUBDB, swap);
        put_u32(&mut page, 88, root, swap);
        page
    }

    pub(crate) fn overflow_page(page_num: u32, data: &[u8], next: u32, swap: bool) -> Vec<u8> {
        assert!(26 + data.len() <= PAGE);
        let mut page = vec![0u8; PAGE];
        data_header(
            &mut page,
            DataHead {
                page_num,
                entries: 0,
                hf: data.len() as u16,
                level: 0,
                kind: super::TYPE_OVERFLOW,
                next,
            },
            swap,
        );
        page[26..26 + data.len()].copy_from_slice(data);
        page
    }

    /// Outer meta, outer leaf (`main` → inner meta), inner meta, then `rest`.
    pub(crate) fn assemble(rest: &[Vec<u8>], swap: bool) -> Vec<u8> {
        let last = (2 + rest.len()) as u32;
        let mut file = Vec::new();
        file.extend_from_slice(&meta(0, 1, last, swap));
        let loc = 2u32.to_be_bytes();
        let outer = vec![
            data_item(b"main", false, swap),
            data_item(&loc, false, swap),
        ];
        file.extend_from_slice(&pack(1, &outer, super::TYPE_LEAF, 1, 0, swap));
        file.extend_from_slice(&meta(2, 3, last, swap));
        for page in rest {
            file.extend_from_slice(page);
        }
        file
    }

    pub(crate) fn pairs(pairs: &[(&[u8], &[u8])], swap: bool) -> Vec<u8> {
        let mut items = Vec::with_capacity(pairs.len() * 2);
        for (k, v) in pairs {
            items.push(data_item(k, false, swap));
            items.push(data_item(v, false, swap));
        }
        assemble(&[pack(3, &items, super::TYPE_LEAF, 1, 0, swap)], swap)
    }
}
