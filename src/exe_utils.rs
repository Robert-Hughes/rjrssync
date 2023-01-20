// The only crate I could find that allows read/modify/write of an exe (PE) file, seemed to produce
// corrupt results when running on Linux (maybe I just wasn't using it properly), so we've got our own code here as it's
// not too complicated.

use std::ops::{Sub, Div, Add, Mul};

pub fn extract_section_from_pe(mut pe_bytes: Vec<u8>, section_name: &str) -> Result<Vec<u8>, String> {
    // https://0xrick.github.io/win-internals/pe5/
    // https://learn.microsoft.com/en-us/windows/win32/debug/pe-format

    // Skip past the DOS header to the PE signature/magic
    //TODO: share common code with the add_section function?
    let signature_offset = read_field::<u32>(&pe_bytes, 0x3c)? as usize;
    let signature = read_field::<u32>(&pe_bytes, signature_offset)?;
    if signature.to_le_bytes() != *b"PE\0\0" {
        return Err(format!("Invalid PE signature"));
    }
    let file_header_offset = signature_offset + 4;
    let num_sections = read_field::<u16>(&pe_bytes, file_header_offset + 2)?;
    let size_of_optional_header = read_field::<u16>(&pe_bytes, file_header_offset + 16)?;

    let optional_header_offset = file_header_offset + 20;

    let section_headers_offset = optional_header_offset + size_of_optional_header as usize;

    for section_idx in 0..num_sections {
        let section_header_offset = section_headers_offset + section_idx as usize * 40;
        let name_offset = section_header_offset + 0; // Name is the first field
        let name = read_string(&pe_bytes, name_offset, 8)?;

        if name == section_name {
            // This is the right section - return the contents
            let size_of_raw_data = read_field::<u32>(&pe_bytes, section_header_offset + 16)?;
            let pointer_to_raw_data = read_field::<u32>(&pe_bytes, section_header_offset + 20)?;
            let mut x = pe_bytes.split_off(pointer_to_raw_data as usize);
            x.truncate(size_of_raw_data as usize);
            return Ok(x);
        }       
    }
    
    Err(format!("Can't find section with name '{section_name}'"))
}

pub fn add_section_to_pe(mut pe_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>) 
    -> Result<Vec<u8>, String> {
    let new_section_size = new_section_bytes.len();

    let signature_offset = read_field::<u32>(&pe_bytes, 0x3c)? as usize;
    let signature = read_field::<u32>(&pe_bytes, signature_offset)?;
    if signature.to_le_bytes() != *b"PE\0\0" {
        return Err(format!("Invalid PE signature"));
    }
    let file_header_offset = signature_offset + 4;
    
    // Increment number of sections
    let num_sections_offset = file_header_offset + 2;
    let orig_num_sections = read_field::<u16>(&pe_bytes, num_sections_offset)?;
    let new_num_sections = orig_num_sections + 1;
    write_field(&mut pe_bytes, num_sections_offset, new_num_sections)?;

    let size_of_optional_header = read_field::<u16>(&pe_bytes, file_header_offset + 16)?;

    let optional_header_offset = file_header_offset + 20;
    let section_alignment = read_field::<u32>(&pe_bytes, optional_header_offset + 32)?;
    let file_alignment = read_field::<u32>(&pe_bytes, optional_header_offset + 36)?;

    let section_headers_offset = optional_header_offset + size_of_optional_header as usize;

    // Because the section data is aligned to FileAlignment, there is (probably) a gap of padding after
    // the end of the section headers and before the data. There we can put our new section header in there
    // without having to shuffle everything else up, if there is such a gap. If not, we will have to shuffle everything
    // up by one FileAlignment.
    let orig_end_of_section_headers = section_headers_offset + orig_num_sections as usize * 40;
    if align(orig_end_of_section_headers as u32, file_alignment) - (orig_end_of_section_headers as u32) < 40 {
        // No space, we'll need to bump everything up
        let padding = vec![0 as u8; file_alignment as usize];
        pe_bytes.splice(orig_end_of_section_headers..orig_end_of_section_headers, padding).for_each(drop);
       
        // All the existing sections need their PointerToRawData offsetting to point to the offseted data
        for section_idx in 0..orig_num_sections {
            let section_header_offset = section_headers_offset + section_idx as usize * 40;

            let pointer_to_raw_data_offset = section_header_offset + 20;
            let orig_pointer_to_raw_data = read_field::<u32>(&pe_bytes, pointer_to_raw_data_offset)?;
            let new_pointer_to_raw_data = orig_pointer_to_raw_data + file_alignment;
            write_field(&mut pe_bytes, pointer_to_raw_data_offset, new_pointer_to_raw_data as u32)?;
        }
    }

    // Create the new section header
    assert!(new_section_name.len() <= 8);
    let mut new_section_header = [0 as u8; 40];
    // Name
    new_section_header[0..8].copy_from_slice(new_section_name.as_bytes());
    // VirtualSize - this has to be set to non-zero otherwise the exe is not valid. We don't actually want this data
    // loaded into memory though, so we set this as small as possible (not sure if this actually achieves anything or not though)
    let new_section_virtual_size = 0x1000;
    write_field(&mut new_section_header, 8, new_section_virtual_size as u32)?;
    // VirtualAddress - we don't really want our data loaded into memory, but this is required, and it seems it 
    // must come contigusouly after the VAs for every preceding section, aligned to SectionAlignment
    let prev_section_virtual_address_offset = section_headers_offset as usize + (orig_num_sections as usize - 1) * 40 + 12;
    let prev_section_virtual_address = read_field::<u32>(&pe_bytes, prev_section_virtual_address_offset)?;
    let prev_section_virtual_size_offset = section_headers_offset as usize + (orig_num_sections as usize - 1) * 40 + 8;
    let prev_section_virtual_size = read_field::<u32>(&pe_bytes, prev_section_virtual_size_offset)?;
    let new_section_virtual_address = align(prev_section_virtual_address + prev_section_virtual_size, section_alignment);
    write_field(&mut new_section_header, 12, new_section_virtual_address as u32)?;
    // SizeOfRawData
    write_field(&mut new_section_header, 16, new_section_size as u32)?;
    // Characteristics
    write_field(&mut new_section_header, 36, 0x00000040u32)?; // IMAGE_SCN_CNT_INITIALIZED_DATA

    // Add the new section header, overwriting the padding/zeroes that we've ensured is there
    let new_section_header_offset = section_headers_offset + orig_num_sections as usize * 40;
    pe_bytes[new_section_header_offset..new_section_header_offset+40].copy_from_slice(&new_section_header);
    
    // Append the new data, adding padding to align it if necessary
    let new_section_offset = align(pe_bytes.len() as u32, file_alignment as u32);
    pe_bytes.resize(new_section_offset as usize, 0);
    pe_bytes.append(&mut new_section_bytes);
    drop(new_section_bytes); // It's just been emptied, so prevent further use

    // Set the new section's PointerToRawData
    write_field(&mut pe_bytes, new_section_header_offset + 20, new_section_offset)?; 
    
    // Update SizeOfImage in the optional header
    let size_of_image_offset = optional_header_offset + 56;
    let new_size_of_image = new_section_virtual_address + new_section_virtual_size as u32;
    let new_size_of_image = align(new_size_of_image, section_alignment);
    write_field(&mut pe_bytes, size_of_image_offset, new_size_of_image as u32)?;    

    // Recalculate SizeOfHeaders in the optional header
    let size_of_headers_offset = optional_header_offset + 60;
    let new_size_of_headers = orig_end_of_section_headers + 40;
    let new_size_of_headers = align(new_size_of_headers as u32, file_alignment);
    write_field(&mut pe_bytes, size_of_headers_offset, new_size_of_headers as u32)?;    
    
    Ok(pe_bytes)
}

// There doesn't seem to be a crate which allows easily adding a section to an existing ELF file.
// They either only support reading (not editing/writing), or do support writing but you have
// to declare your ELF file from scratch (no read/modify/write). The one crate that does do this
// (elf_utilities) seems to have a bug and it produced corrupted ELFs :(.
// So we do it ourselves in this code.

#[cfg_attr(not(unix), allow(unused))]
pub fn extract_section_from_elf(mut elf_bytes: Vec<u8>, section_name: &str) -> Result<Vec<u8>, String> {
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    //TODO: validate endian-ness field?
    //TODO: validate bitness (64 vs 32), and ELF magic number?

    // Offset within file to the start of the table of section headers
    let section_header_table_offset = read_field::<u64>(&elf_bytes, 0x28)?;
    let num_sections = read_field::<u16>(&elf_bytes, 0x3C)?;
    // Size of each section header
    let section_header_size = read_field::<u16>(&elf_bytes, 0x3A)?;

    // Find the string table that contains section names
    let section_names_section_idx = read_field::<u16>(&elf_bytes, 0x3E)?;
    let section_names_table_offset = read_field::<u64>(&elf_bytes, 
        section_header_table_offset as usize + section_names_section_idx as usize * section_header_size as usize + 0x18)?;


    // Find the right section
    for section_idx in 0..num_sections {
        // Read the section name and check it
        let name_offset_within_string_table = read_field::<u32>(&elf_bytes, 
            section_header_table_offset as usize + section_idx as usize * section_header_size as usize + 0x0)?;
        let mut char_idx = section_names_table_offset as usize + name_offset_within_string_table as usize;
        //TODO: must be something better
        // use our read_string
        let mut name_str = String::new();
        loop {
            let c = elf_bytes[char_idx];
            char_idx += 1;
            if c == b'\0' {
                break;
            }
            name_str.push(c as char);
        }
        if name_str == section_name {
            // This is the right section - return the contents
            let section_data_offset = read_field::<u64>(&elf_bytes, 
                section_header_table_offset as usize + section_idx as usize * section_header_size as usize + 0x18)?;
            let section_data_size = read_field::<u64>(&elf_bytes, 
                section_header_table_offset as usize + section_idx as usize * section_header_size as usize + 0x20)?;
            let mut x = elf_bytes.split_off(section_data_offset as usize);
            x.truncate(section_data_size as usize);
            return Ok(x);
        }       
    }

    Err(format!("Can't find section with name '{section_name}'"))
}

pub fn add_section_to_elf(mut elf_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>) 
    -> Result<Vec<u8>, String> {
    let new_section_size = new_section_bytes.len();
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    //TODO: validate endian-ness field?
    //TODO: validate bitness (64 vs 32), and ELF magic number?
    //TODO: common ELF code to do this, as well as the common parsing code?

    // Offset within file to the start of the table of section headers
    let section_header_table_offset = read_field::<u64>(&elf_bytes, 0x28)?;
    // Size of each section header
    let section_header_size = read_field::<u16>(&elf_bytes, 0x3A)?;
    
    // Number of sections - we'll need to increment this, but we'll save the value later
    let orig_num_sections = read_field::<u16>(&elf_bytes, 0x3C)?;
    let new_num_sections = orig_num_sections + 1;

    // Index of the section which contains sections names. We'll need to update this section
    // to include the name of our new section.
    let section_names_section_idx = read_field::<u16>(&elf_bytes, 0x3E)?;

    // Remove the section header table and keep it separate, as once we start modifying the file we'll overwrite this.
    // We'll add the table back at the end
    if elf_bytes.len() != section_header_table_offset as usize + orig_num_sections as usize * section_header_size as usize {
        return Err(format!("Elf file wrong size"));
    }
    let mut section_header_table = elf_bytes.split_off(section_header_table_offset as usize);
        
    // Update the section which contains section header names, adding our new section name
    let section_names_table_offset = read_field::<u64>(&section_header_table, 
        section_names_section_idx as usize * section_header_size as usize + 0x18)?;
    let section_names_table_old_size = read_field::<u64>(&section_header_table, 
        section_names_section_idx as usize * section_header_size as usize + 0x20)?;
    // Add new bytes to the end
    let mut new_bytes = new_section_name.as_bytes().to_vec();
    new_bytes.push(b'\0');
    let inserted_name_num_bytes = new_bytes.len();
    elf_bytes.splice(
        section_names_table_offset as usize + section_names_table_old_size as usize..
        section_names_table_offset as usize + section_names_table_old_size as usize,
        new_bytes).for_each(drop);

    // Update names section size
    let section_names_table_new_size = section_names_table_old_size + inserted_name_num_bytes as u64;
    write_field::<u64>(&mut section_header_table,
        section_names_section_idx as usize * section_header_size as usize + 0x20, section_names_table_new_size)?;

    // Update any of the sections following the section header names section, as their offsets
    // will have changed now that we inserted data
    for section_idx in section_names_section_idx + 1..orig_num_sections {
        let offset = read_field::<u64>(&section_header_table, 
            section_idx as usize * section_header_size as usize + 0x18)?;
        let new_offset = offset + inserted_name_num_bytes as u64;
        write_field::<u64>(&mut section_header_table,
            section_idx as usize * section_header_size as usize + 0x18, new_offset)?;
    }

    // Add our new section data!
    let new_section_offset = elf_bytes.len();
    elf_bytes.append(&mut new_section_bytes);
    drop(new_section_bytes); // It's just been emptied, so prevent further use

    // Add entry to the section header table for our new section
    let mut new_section_header = vec![0 as u8; section_header_size as usize];
    // sh_name
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

    // Update the main header with the new offset of the section table and the new number of sections
    write_field::<u64>(&mut elf_bytes, 0x28, new_section_header_table_offset)?;
    write_field::<u16>(&mut elf_bytes, 0x3C, new_num_sections)?;

    Ok(elf_bytes)
}

// Convenient functions to read/write fields from a byte array.

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

fn read_field<T: Number>(bytes: &[u8], offset: usize) -> Result<T, String> {
    let size = std::mem::size_of::<T>();
    let b = bytes.get(offset..offset + size).ok_or(format!("Failed to read {size} bytes at {offset}"))?;
    let x = T::from_bytes(b);
    Ok(x)
}

fn write_field<T: Number>(bytes: &mut [u8], offset: usize, val: T) -> Result<(), String> {
    let size = std::mem::size_of::<T>();
    let b = bytes.get_mut(offset..offset + size).ok_or(format!("Failed to read {size} bytes at {offset}"))?;
    b.copy_from_slice(&val.to_bytes());
    Ok(())
}

fn read_string(bytes: &[u8], mut offset: usize, max_size: usize) -> Result<String, String> {
    let mut result = String::new();
    loop {
        let c = *bytes.get(offset).ok_or(format!("Failed to read string at offset {offset}"))?;
        offset += 1;
        if c == b'\0' || result.len() >= max_size {
            break;
        }
        result.push(c as char);
    }
    Ok(result)
}

fn align<T: Copy + Sub<Output = T> + Div<Output = T> + Add<Output=T> + Mul<Output = T> + From<u32>>(x: T, multiple: T) -> T{
    ((x - 1.into()) / multiple + 1.into()) * multiple
}

//TODO: tests for this file