pub fn add_section_to_elf(mut elf_bytes: Vec<u8>, new_section_name: &str, mut new_section_bytes: Vec<u8>) 
    -> Result<Vec<u8>, String> {
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format

    //TODO: Range is start..end, not size!

    // Offset within file to the start of the table of section headers
    let section_header_table_offset = u64::from_le_bytes(elf_bytes.get(0x28..8).ok_or(format!("Elf file too short"))?.try_into().unwrap());
    // Size of each section header
    let section_header_size = u16::from_le_bytes(elf_bytes.get(0x3A..2).ok_or(format!("Elf file too short"))?.try_into().unwrap());
    
    // Number of sections - we'll need to increment this, but we'll save the value later
    let orig_num_sections = u16::from_le_bytes(elf_bytes.get(0x3C..2).ok_or(format!("Elf file too short"))?.try_into().unwrap());
    let new_num_sections = orig_num_sections + 1;

    // Index of the section which contains sections names. We'll need to update this section
    // to include the name of our new section.
    let section_names_section_idx = u16::from_le_bytes(elf_bytes.get(0x3E..2).ok_or(format!("Elf file too short"))?.try_into().unwrap());

    // Remove the section header table and keep it separate, as once we start modifying the file we'll overwrite this.
    // We'll add the table back at the end
    if elf_bytes.len() != section_header_table_offset as usize + orig_num_sections as usize * section_header_size as usize {
        return Err(format!("Elf file wrong size"));
    }
    let mut section_header_table = elf_bytes.split_off(section_header_table_offset as usize);
        
    // Update the section which contains section header names, adding our new section name
    let section_names_table_offset = u64::from_le_bytes(section_header_table.get(
        section_names_section_idx as usize * section_header_size as usize + 0x18..8).ok_or(format!("Elf file too short"))?.try_into().unwrap());        
    let section_names_table_size = u64::from_le_bytes(section_header_table.get(
        section_names_section_idx as usize * section_header_size as usize + 0x20..8).ok_or(format!("Elf file too short"))?.try_into().unwrap());
    // Add new bytes to the end
    let mut new_bytes = new_section_name.as_bytes().to_vec();
    new_bytes.push(b'\0');
    let inserted_name_num_bytes = new_bytes.len();
    elf_bytes.splice(
        section_names_table_offset as usize + section_names_table_size as usize..
        section_names_table_offset as usize + section_names_table_size as usize,
        new_bytes).collect::<Vec<u8>>();

    // Update any of the sections following the section header names section, as their offsets
    // will have changed now that we inserted data
    for section_idx in section_names_section_idx + 1..orig_num_sections {
        let offset = u64::from_le_bytes(section_header_table.get(
            section_idx as usize * section_header_size as usize + 0x18..8).ok_or(format!("Elf file too short"))?.try_into().unwrap());        
        let new_offset = offset + inserted_name_num_bytes as u64;
        section_header_table.get_mut(
            section_idx as usize * section_header_size as usize + 0x18..8).unwrap()
            .copy_from_slice(&new_offset.to_le_bytes());
    }

    // Add our new section data!
    elf_bytes.append(&mut new_section_bytes);

    // Add entry to the section header table for our new section

    // Append updated section table
    let new_section_header_table_offset = elf_bytes.len() as u64;
    elf_bytes.append(&mut section_header_table);

    // Update the main header with the new offset of the section table and the new number of sections
    elf_bytes.get_mut(0x28..8).unwrap().copy_from_slice(&new_section_header_table_offset.to_le_bytes());
    elf_bytes.get_mut(0x3C..2).unwrap().copy_from_slice(&new_num_sections.to_le_bytes());

    Ok(elf_bytes)
}

// fn modify_field(bytes: &mut [u8], offset: usize, size: usize) -> Result< {
//     let x = u16::from_le_bytes(bytes.get(0x3C..2).ok_or(format!("Elf file too short"))?.try_into().unwrap());

// }

