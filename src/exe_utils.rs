// There doesn't seem to be a crate which allows easily adding a section to an existing ELF file.
// They either only support reading (not editing/writing), or do support writing but you have
// to declare your ELF file from scratch (no read/modify/write). The one crate that does do this
// (elf_utilities) seems to have a bug and it produced corrupted ELFs :(.
// So we do it ourselves in this code.

pub fn add_section_to_elf(mut elf_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>) 
    -> Result<Vec<u8>, String> {
    let new_section_size = new_section_bytes.len();
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    //TODO: validate endian-ness field?

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
    drop(new_section_bytes); // It's just been emptied

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

#[cfg_attr(not(unix), allow(unused))]
pub fn extract_section_from_elf(mut elf_bytes: Vec<u8>, section_name: &str) -> Result<Vec<u8>, String> {
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
    // https://docs.oracle.com/cd/E23824_01/html/819-0690/chapter6-73709.html

    //TODO: validate endian-ness field?

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

    Err(format!("Can't find the section"))
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

//TODO: tests for this file