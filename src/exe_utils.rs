// Functions for extracting and modifying sections in executable file formats.

// For Windows, the only crate I could find that allows read/modify/write of an exe (PE) file, seemed to produce
// corrupt results when running on Linux (maybe I just wasn't using it properly), so we've got our own code here as it's
// not too complicated.

// There doesn't seem to be a crate which allows easily adding a section to an existing ELF file.
// They either only support reading (not editing/writing), or do support writing but you have
// to declare your ELF file from scratch (no read/modify/write). The one crate that does do this
// (elf_utilities) seems to have a bug and it produced corrupted ELFs :(.
// So we do it ourselves in this code.

use std::ops::{Sub, Div, Add, Mul};

pub enum ExtractSectionError {
    SectionNotFound,
    Other(String),
}
impl From<String> for ExtractSectionError {
    fn from(s: String) -> Self {
        ExtractSectionError::Other(s)
    }
}

// Validates the magic signature and returns the offset of the start of the COFF file header.
#[cfg_attr(not(windows), allow(unused))]
fn validate_pe_signature(pe_bytes: &[u8]) -> Result<usize, String> {
    // PE files are always little-endian: https://reverseengineering.stackexchange.com/questions/17922/determining-endianness-of-pe-files-windows-on-arm
    // The code written here assumes that we are running on a little-endian system too, so confirm this.
    if 0x12345678_u32.to_le_bytes() != 0x12345678_u32.to_ne_bytes() {
        return Err(format!("This code can only run on a little-endian system"));
    }

    let signature_offset = read_field::<u32>(&pe_bytes, 0x3c)? as usize;
    let signature = read_field::<u32>(&pe_bytes, signature_offset)?;
    if signature.to_le_bytes() != *b"PE\0\0" {
        return Err(format!("Invalid PE signature"));
    }

    // COFF file header comes immediately after the signature
    Ok(signature_offset + 4)
}

#[cfg_attr(not(windows), allow(unused))]
pub fn extract_section_from_pe(mut pe_bytes: Vec<u8>, section_name: &str) -> Result<Vec<u8>, ExtractSectionError> {
    // https://0xrick.github.io/win-internals/pe5/
    // https://learn.microsoft.com/en-us/windows/win32/debug/pe-format

    // Skip past the DOS header to the COFF header
    let file_header_offset = validate_pe_signature(&pe_bytes)?;

    let num_sections = read_field::<u16>(&pe_bytes, file_header_offset + 2)?;
    let size_of_optional_header = read_field::<u16>(&pe_bytes, file_header_offset + 16)?;

    let optional_header_offset = file_header_offset + 20;

    let section_headers_offset = optional_header_offset + size_of_optional_header as usize;

    // Search through the section table for the one with the right name
    for section_idx in 0..num_sections {
        let section_header_offset = section_headers_offset + section_idx as usize * 40;
        let name_offset = section_header_offset + 0; // Name is the first field
        let name = read_string(&pe_bytes, name_offset, 8)?;

        if name == section_name.as_bytes() {
            // This is the right section - return the contents
            let size_of_raw_data = read_field::<u32>(&pe_bytes, section_header_offset + 16)?;
            let pointer_to_raw_data = read_field::<u32>(&pe_bytes, section_header_offset + 20)?;
            let mut x = pe_bytes.split_off(pointer_to_raw_data as usize);
            x.truncate(size_of_raw_data as usize);
            return Ok(x);
        }
    }

    Err(ExtractSectionError::SectionNotFound)
}

#[cfg_attr(not(windows), allow(unused))]
pub fn add_section_to_pe(mut pe_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>)
    -> Result<Vec<u8>, String>
{
    // Skip past the DOS header to the COFF header
    let file_header_offset = validate_pe_signature(&pe_bytes)?;

    // Increment number of sections
    let num_sections_offset = file_header_offset + 2;
    let orig_num_sections = read_field::<u16>(&pe_bytes, num_sections_offset)?;
    let new_num_sections = orig_num_sections + 1;
    write_field::<u16>(&mut pe_bytes, num_sections_offset, new_num_sections)?;

    let size_of_optional_header = read_field::<u16>(&pe_bytes, file_header_offset + 16)?;

    let optional_header_offset = file_header_offset + 20;

    let section_alignment = read_field::<u32>(&pe_bytes, optional_header_offset + 32)?;
    let file_alignment = read_field::<u32>(&pe_bytes, optional_header_offset + 36)?;

    let section_headers_offset = optional_header_offset + size_of_optional_header as usize;

    // Because the section data is aligned to FileAlignment, there is (probably) a gap of padding after
    // the end of the section headers and before the data. We can put our new section header in there
    // without having to shuffle everything else up, if there is such a gap. If not, we will have to
    // shuffle everything up by one FileAlignment, to create space for our new section header.
    let orig_end_of_section_headers = section_headers_offset + orig_num_sections as usize * 40;
    if align(orig_end_of_section_headers, file_alignment as usize) - orig_end_of_section_headers < 40 {
        // No space, we'll need to bump everything up
        let padding = vec![0 as u8; file_alignment as usize];
        pe_bytes.splice(orig_end_of_section_headers..orig_end_of_section_headers, padding).for_each(drop);

        // All the existing sections need their PointerToRawData offsetting to point to the offseted data
        for section_idx in 0..orig_num_sections {
            let section_header_offset = section_headers_offset + section_idx as usize * 40;

            let pointer_to_raw_data_offset = section_header_offset + 20;
            let orig_pointer_to_raw_data = read_field::<u32>(&pe_bytes, pointer_to_raw_data_offset)?;
            let new_pointer_to_raw_data = orig_pointer_to_raw_data + file_alignment;
            write_field::<u32>(&mut pe_bytes, pointer_to_raw_data_offset, new_pointer_to_raw_data as u32)?;
        }
    }

    // Create the new section header
    assert!(new_section_name.len() <= 8);
    let mut new_section_header = [0 as u8; 40];
    // Name
    new_section_header[0..new_section_name.len()].copy_from_slice(new_section_name.as_bytes());
    // VirtualSize - this has to be set to non-zero otherwise the exe is not valid. We don't actually want this data
    // loaded into memory though, so we set this as small as possible (not sure if this actually achieves anything or not though).
    // Note that this doesn't need to be aligned.
    let new_section_virtual_size = 0x1;
    write_field::<u32>(&mut new_section_header, 8, new_section_virtual_size as u32)?;
    // VirtualAddress - we don't really want our data loaded into memory, but this is required, and it must
    // be contigus after the VAs for every preceding section, aligned to SectionAlignment
    let prev_section_virtual_address_offset = section_headers_offset as usize + (orig_num_sections as usize - 1) * 40 + 12;
    let prev_section_virtual_address = read_field::<u32>(&pe_bytes, prev_section_virtual_address_offset)?;
    let prev_section_virtual_size_offset = section_headers_offset as usize + (orig_num_sections as usize - 1) * 40 + 8;
    let prev_section_virtual_size = read_field::<u32>(&pe_bytes, prev_section_virtual_size_offset)?;
    let new_section_virtual_address = align(prev_section_virtual_address + prev_section_virtual_size, section_alignment);
    write_field::<u32>(&mut new_section_header, 12, new_section_virtual_address as u32)?;
    // SizeOfRawData
    let new_section_size_of_raw_data = align(new_section_bytes.len() as u32, file_alignment);
    write_field::<u32>(&mut new_section_header, 16, new_section_size_of_raw_data)?;
    // Characteristics
    write_field::<u32>(&mut new_section_header, 36, 0x00000040)?; // IMAGE_SCN_CNT_INITIALIZED_DATA

    // Add the new section header, overwriting the padding/zeroes that we've ensured is there
    let new_section_header_offset = section_headers_offset + orig_num_sections as usize * 40;
    pe_bytes[new_section_header_offset..new_section_header_offset+40].copy_from_slice(&new_section_header);

    // Append the new section data, adding padding before to align it if necessary,
    // and padding after to make it match SizeOfRawData
    let new_section_offset = align(pe_bytes.len(), file_alignment as usize);
    pe_bytes.resize(new_section_offset as usize, 0);
    new_section_bytes.resize(new_section_size_of_raw_data as usize, 0);
    pe_bytes.append(&mut new_section_bytes);
    drop(new_section_bytes); // It's just been emptied, so prevent further use

    // Set the new section's PointerToRawData
    write_field::<u32>(&mut pe_bytes, new_section_header_offset + 20, new_section_offset as u32)?;

    // Update SizeOfImage in the optional header
    let size_of_image_offset = optional_header_offset + 56;
    let new_size_of_image = new_section_virtual_address + new_section_virtual_size as u32;
    let new_size_of_image = align(new_size_of_image, section_alignment);
    write_field::<u32>(&mut pe_bytes, size_of_image_offset, new_size_of_image as u32)?;

    // Recalculate SizeOfHeaders in the optional header
    let size_of_headers_offset = optional_header_offset + 60;
    let new_size_of_headers = orig_end_of_section_headers + 40;
    let new_size_of_headers = align(new_size_of_headers, file_alignment as usize);
    write_field::<u32>(&mut pe_bytes, size_of_headers_offset, new_size_of_headers as u32)?;

    Ok(pe_bytes)
}

#[cfg_attr(not(unix), allow(unused))]
fn validate_elf_header(elf_bytes: &[u8]) -> Result<(), String> {
    // ELF files can be big or little endian.
    // The code written here assumes that it is little-endian and that we are
    // running on a little-endian system too, so confirm this.
    if 0x12345678_u32.to_le_bytes() != 0x12345678_u32.to_ne_bytes() {
        return Err(format!("This code can only run on a little-endian system"));
    }

    let magic = read_field::<u32>(&elf_bytes, 0)?;
    if magic.to_le_bytes() != *b"\x7FELF" {
        return Err(format!("Invalid ELF magic"));
    }

    let bitness = read_field::<u8>(&elf_bytes, 0x4)?;
    if bitness != 2 {
        // There's some minor differences, so we could support 32-bit, but no need at the moment.
        return Err(format!("Only 64-bit ELF files are supported"));
    }

    let endianness = read_field::<u8>(&elf_bytes, 0x5)?;
    if endianness != 1 {
        return Err(format!("Only little-endian ELF files are supported"));
    }

    let version = read_field::<u8>(&elf_bytes, 0x6)?;
    if version != 1 {
        return Err(format!("Only V1 ELF files are supported"));
    }

    Ok(())
}

#[cfg_attr(not(unix), allow(unused))]
pub fn extract_section_from_elf(mut elf_bytes: Vec<u8>, section_name: &str) -> Result<Vec<u8>, ExtractSectionError> {
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    validate_elf_header(&elf_bytes)?;

    // Offset within file to the start of the table of section headers
    let section_header_table_offset = read_field::<u64>(&elf_bytes, 0x28)? as usize;
    // Size of each section header
    let section_header_size = read_field::<u16>(&elf_bytes, 0x3A)? as usize;
    let num_sections = read_field::<u16>(&elf_bytes, 0x3C)? as usize;

    // Find the string table that contains section names (which is itself a section)
    let section_names_section_idx = read_field::<u16>(&elf_bytes, 0x3E)? as usize;
    let section_names_table_offset = read_field::<u64>(&elf_bytes,
        section_header_table_offset + section_names_section_idx * section_header_size + 0x18)? as usize;

    // Search through the section table for the one with the right name
    for section_idx in 0..num_sections {
        let section_header_offset = section_header_table_offset + section_idx * section_header_size;
        // Read the section name and check it. Name is the first field (offset 0x0)
        let name_offset_within_string_table =
            read_field::<u32>(&elf_bytes, section_header_offset + 0x0)? as usize;
        // Practical max length of 32, just to prevent us reading too much garbage if something goes wrong
        let name = read_string(&elf_bytes, section_names_table_offset + name_offset_within_string_table, 32)?;
        if name == section_name.as_bytes() {
            // This is the right section - return the contents
            let section_data_offset = read_field::<u64>(&elf_bytes, section_header_offset + 0x18)?;
            let section_data_size = read_field::<u64>(&elf_bytes, section_header_offset + 0x20)?;

            let mut x = elf_bytes.split_off(section_data_offset as usize);
            x.truncate(section_data_size as usize);
            return Ok(x);
        }
    }

    Err(ExtractSectionError::SectionNotFound)
}

#[cfg_attr(not(unix), allow(unused))]
pub fn add_section_to_elf(mut elf_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>)
    -> Result<Vec<u8>, String>
{
    let new_section_size = new_section_bytes.len();
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    validate_elf_header(&elf_bytes)?;

    // Offset within file to the start of the table of section headers
    let section_header_table_offset = read_field::<u64>(&elf_bytes, 0x28)? as usize;
    // Size of each section header
    let section_header_size = read_field::<u16>(&elf_bytes, 0x3A)? as usize;

    // Number of sections - we'll need to increment this, but we'll save the new value later
    let orig_num_sections = read_field::<u16>(&elf_bytes, 0x3C)? as usize;
    let new_num_sections = orig_num_sections + 1;

    // Index of the section which contains sections names.
    // We'll need to update the contents this section to include the name of our new section.
    let section_names_section_idx = read_field::<u16>(&elf_bytes, 0x3E)? as usize;

    // Remove the section header table and keep it separate, otherwise once we start
    // modifying the file we would overwrite this. We'll add the table back once we're done.
    if elf_bytes.len() != section_header_table_offset + orig_num_sections * section_header_size {
        return Err(format!("ELF file wrong size or layout"));
    }
    let mut section_header_table = elf_bytes.split_off(section_header_table_offset);

    // Update the section which contains section header names, appending our new section name
    let section_names_table_offset = read_field::<u64>(&section_header_table,
        section_names_section_idx * section_header_size + 0x18)? as usize;
    let section_names_table_old_size = read_field::<u64>(&section_header_table,
        section_names_section_idx * section_header_size + 0x20)? as usize;
    // Add new bytes to the end
    let mut new_bytes = new_section_name.as_bytes().to_vec();
    new_bytes.push(b'\0');
    let inserted_name_num_bytes = new_bytes.len();
    elf_bytes.splice(
        section_names_table_offset + section_names_table_old_size..
        section_names_table_offset + section_names_table_old_size,
        new_bytes).for_each(drop);

    // Update names section size in its section header
    let section_names_table_new_size = section_names_table_old_size + inserted_name_num_bytes;
    write_field::<u64>(&mut section_header_table,
        section_names_section_idx * section_header_size + 0x20, section_names_table_new_size as u64)?;

    // Update the section headers for any sections following the names section,
    // as their offsets will have been bumped up now that we inserted data
    for section_idx in section_names_section_idx + 1..orig_num_sections {
        let section_offset_offset = section_idx * section_header_size + 0x18;
        let orig_offset = read_field::<u64>(&section_header_table, section_offset_offset)?;
        let new_offset = orig_offset + inserted_name_num_bytes as u64;
        write_field::<u64>(&mut section_header_table, section_offset_offset, new_offset)?;
    }

    // Add our new section data!
    let new_section_offset = elf_bytes.len();
    elf_bytes.append(&mut new_section_bytes);
    drop(new_section_bytes); // It's just been emptied, so prevent further use

    // Add header for our new section to the section header table
    let mut new_section_header = vec![0 as u8; section_header_size];
    // sh_name - the offset into the names section for our new name
    write_field::<u32>(&mut new_section_header, 0x0, section_names_table_old_size as u32)?;
    // sh_type
    write_field::<u32>(&mut new_section_header, 0x04, 0x80000000)?; // 0x80000000 (and above) is for custom sections
    // sh_offset
    write_field::<u64>(&mut new_section_header, 0x18, new_section_offset as u64)?;
    // sh_size
    write_field::<u64>(&mut new_section_header, 0x20, new_section_size as u64)?;
    section_header_table.append(&mut new_section_header);

    // Append updated section table
    let new_section_header_table_offset = elf_bytes.len() as u64;
    elf_bytes.append(&mut section_header_table);
    drop(section_header_table); // It's just been emptied, so prevent further use

    // Update the main header with the new offset of the section table and the new number of sections
    write_field::<u64>(&mut elf_bytes, 0x28, new_section_header_table_offset)?;
    write_field::<u16>(&mut elf_bytes, 0x3C, new_num_sections as u16)?;

    Ok(elf_bytes)
}

/// Reads a fixed-size field (u32, u64, etc.) from a byte array.
fn read_field<T: Number>(bytes: &[u8], offset: usize) -> Result<T, String> {
    let size = std::mem::size_of::<T>();
    let b = bytes.get(offset..offset + size).ok_or(format!("Failed to read {size} bytes at {offset}"))?;
    let x = T::from_bytes(b);
    Ok(x)
}

/// Writes a fixed-size field (u32, u64, etc.) into a byte array.
fn write_field<T: Number>(bytes: &mut [u8], offset: usize, val: T) -> Result<(), String> {
    let size = std::mem::size_of::<T>();
    let b = bytes.get_mut(offset..offset + size).ok_or(format!("Failed to write {size} bytes at {offset}"))?;
    b.copy_from_slice(&val.to_bytes());
    Ok(())
}

/// Reads a null-terminated string from a byte array, starting at the given offset.
/// Stops after reading `max_size` bytes, if this comes before finding the null terminator.
fn read_string(bytes: &[u8], offset: usize, max_size: usize) -> Result<&[u8], String> {
    let mut size = 0;
    loop {
        let c = *bytes.get(offset + size).ok_or(format!("Failed to read string at offset {offset}"))?;
        if c == 0 {
            break;
        }
        size += 1;
        if size >= max_size {
            break;
        }
    }
    Ok(&bytes[offset..offset+size])
}

/// Rounds up `x` to a multiple of `multiple`
fn align<T>(x: T, multiple: T) -> T
    where T : Copy + Sub<Output = T> + Div<Output = T> + Add<Output=T> + Mul<Output = T> + From<u8>
{
    ((x - 1.into()) / multiple + 1.into()) * multiple
}

trait Number {
    fn from_bytes(bytes: &[u8]) -> Self;
    fn to_bytes(self) -> Vec<u8>;
}

macro_rules! impl_number_trait {
    ($T:ty) => {
        impl Number for $T {
            fn from_bytes(bytes: &[u8]) -> Self {
                 Self::from_le_bytes(bytes.try_into().unwrap())
            }
            fn to_bytes(self) -> Vec<u8> {
                self.to_le_bytes().to_vec()
            }
        }
    };
}

impl_number_trait!(u64);
impl_number_trait!(u32);
impl_number_trait!(u16);
impl_number_trait!(u8);
