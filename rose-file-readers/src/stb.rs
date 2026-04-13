use anyhow::{anyhow, ensure};
use core::mem::size_of;
use std::collections::HashMap;

use crate::{reader::RoseFileReader, RoseFile, RoseFileWriter};

pub struct StbFile {
    version: u8,
    row_height: u32,
    column_widths: Vec<u8>,
    column_names: Vec<String>,
    row_name_header: String,
    row_names: Vec<String>,
    cells: Vec<String>,
    row_keys: HashMap<String, usize>,
}

#[derive(Default)]
pub struct StbReadOptions {
    pub is_wide: bool,
    pub with_keys: bool,
}

impl RoseFile for StbFile {
    type ReadOptions = StbReadOptions;
    type WriteOptions = ();

    fn read(
        mut reader: RoseFileReader,
        read_options: &StbReadOptions,
    ) -> Result<Self, anyhow::Error> {
        let magic = reader.read_fixed_length_string(3)?;
        if magic != "STB" {
            return Err(anyhow!("Invalid STB magic header: {}", &magic));
        }

        if read_options.is_wide {
            reader.use_wide_strings = true;
        }

        StbFile::read_data(reader, read_options)
    }

    fn write(
        &self,
        writer: &mut RoseFileWriter,
        _options: &Self::WriteOptions,
    ) -> Result<(), anyhow::Error> {
        writer.buffer.extend_from_slice(b"STB");
        writer.write_u8(match self.version {
            0 => b'0',
            1 => b'1',
            version => return Err(anyhow!("Unsupported STB version: {}", version)),
        });

        let data_position_offset = writer.buffer.len();
        writer.write_u32(0);
        writer.write_u32((self.rows() + 1) as u32);
        writer.write_u32((self.columns() + 1) as u32);
        writer.write_u32(self.row_height);

        writer.buffer.extend_from_slice(&self.column_widths);

        for column_name in &self.column_names {
            writer.write_u16_length_string(column_name);
        }

        writer.write_u16_length_string(&self.row_name_header);
        for row_name in &self.row_names {
            writer.write_u16_length_string(row_name);
        }

        let data_position = writer.buffer.len() as u32;
        writer.buffer[data_position_offset..data_position_offset + 4]
            .copy_from_slice(&data_position.to_le_bytes());

        for cell in &self.cells {
            writer.write_u16_length_string(cell);
        }

        Ok(())
    }
}

#[allow(dead_code)]
impl StbFile {
    pub fn new(
        version: u8,
        row_height: u32,
        column_names: Vec<String>,
        row_name_header: impl Into<String>,
        row_names: Vec<String>,
        cells: Vec<String>,
    ) -> Result<Self, anyhow::Error> {
        ensure!(
            version == 0 || version == 1,
            "Unsupported STB version: {}",
            version
        );
        let columns = column_names.len().saturating_sub(1);
        ensure!(
            cells.len() == row_names.len() * columns,
            "Expected {} STB cells, got {}",
            row_names.len() * columns,
            cells.len()
        );

        let column_widths = if version == 0 {
            vec![0; size_of::<u32>()]
        } else {
            vec![0; size_of::<u16>() * (column_names.len() + 1)]
        };
        let row_keys = Self::build_row_keys(&row_names, true);

        Ok(Self {
            version,
            row_height,
            column_widths,
            column_names,
            row_name_header: row_name_header.into(),
            row_names,
            cells,
            row_keys,
        })
    }

    fn read_data(
        mut reader: RoseFileReader,
        read_options: &StbReadOptions,
    ) -> Result<Self, anyhow::Error> {
        let version = {
            let version = reader.read_u8()?;
            if version == b'0' {
                0
            } else if version == b'1' {
                1
            } else {
                return Err(anyhow!("Unsupported STB version: {}", version));
            }
        };

        let data_position = reader.read_u32()? as u64;
        let row_count = reader.read_u32()? as usize;
        let column_count = reader.read_u32()? as usize;
        let row_height = reader.read_u32()?;

        let column_widths = if version == 0 {
            reader.read_fixed_length_bytes(size_of::<u32>())?.to_vec()
        } else {
            reader
                .read_fixed_length_bytes(size_of::<u16>() * (column_count + 1))?
                .to_vec()
        };

        let mut column_names = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            column_names.push(String::from(reader.read_u16_length_string()?));
        }

        let rows = row_count.saturating_sub(1);
        let columns = column_count.saturating_sub(1);

        let row_name_header = String::from(reader.read_u16_length_string()?);

        let mut row_names = Vec::with_capacity(rows);
        for _ in 0..rows {
            row_names.push(String::from(reader.read_u16_length_string()?));
        }

        let mut cells = Vec::with_capacity(rows * columns);

        reader.set_position(data_position);
        for _ in 0..rows {
            for _ in 0..columns {
                cells.push(String::from(reader.read_u16_length_string()?));
            }
        }

        let row_keys = Self::build_row_keys(&row_names, read_options.with_keys);

        Ok(Self {
            version,
            row_height,
            column_widths,
            column_names,
            row_name_header,
            row_names,
            cells,
            row_keys,
        })
    }

    fn build_row_keys(row_names: &[String], with_keys: bool) -> HashMap<String, usize> {
        let mut row_keys = HashMap::new();
        if with_keys {
            for (index, key) in row_names.iter().enumerate() {
                if !key.is_empty() {
                    row_keys.insert(key.clone(), index);
                }
            }
        }
        row_keys
    }

    fn cell_index(&self, row: usize, column: usize) -> Option<usize> {
        if row >= self.rows() || column >= self.columns() {
            None
        } else {
            Some(row * self.columns() + column)
        }
    }

    fn rebuild_row_keys(&mut self) {
        self.row_keys = Self::build_row_keys(&self.row_names, true);
    }

    pub fn rows(&self) -> usize {
        self.row_names.len()
    }

    pub fn columns(&self) -> usize {
        self.column_names.len().saturating_sub(1)
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn row_height(&self) -> u32 {
        self.row_height
    }

    pub fn lookup_row_name(&self, name: &str) -> Option<usize> {
        self.row_keys.get(name).cloned()
    }

    pub fn try_get_row_name(&self, row: usize) -> Option<&str> {
        self.row_names.get(row).map(String::as_str)
    }

    pub fn get_row_name(&self, row: usize) -> &str {
        self.try_get_row_name(row).unwrap_or("")
    }

    pub fn set_row_name(
        &mut self,
        row: usize,
        value: impl Into<String>,
    ) -> Result<(), anyhow::Error> {
        let row_name = self
            .row_names
            .get_mut(row)
            .ok_or_else(|| anyhow!("Invalid STB row {}", row))?;
        *row_name = value.into();
        self.rebuild_row_keys();
        Ok(())
    }

    pub fn row_name_header(&self) -> &str {
        &self.row_name_header
    }

    pub fn column_name(&self, column: usize) -> Option<&str> {
        self.column_names.get(column).map(String::as_str)
    }

    pub fn try_get(&self, row: usize, column: usize) -> Option<&str> {
        let cell_index = self.cell_index(row, column)?;
        let value = self.cells.get(cell_index)?;
        if value.is_empty() {
            None
        } else {
            Some(value.as_str())
        }
    }

    pub fn get(&self, row: usize, column: usize) -> &str {
        self.try_get(row, column).unwrap_or("")
    }

    pub fn set(
        &mut self,
        row: usize,
        column: usize,
        value: impl Into<String>,
    ) -> Result<(), anyhow::Error> {
        let cell_index = self
            .cell_index(row, column)
            .ok_or_else(|| anyhow!("Invalid STB cell ({}, {})", row, column))?;
        self.cells[cell_index] = value.into();
        Ok(())
    }

    pub fn push_row(
        &mut self,
        row_name: impl Into<String>,
        values: impl IntoIterator<Item = String>,
    ) -> Result<usize, anyhow::Error> {
        let values: Vec<String> = values.into_iter().collect();
        ensure!(
            values.len() == self.columns(),
            "Expected {} STB cells, got {}",
            self.columns(),
            values.len()
        );

        let row_index = self.rows();
        self.row_names.push(row_name.into());
        self.cells.extend(values);
        self.rebuild_row_keys();
        Ok(row_index)
    }

    pub fn try_get_int(&self, row: usize, column: usize) -> Option<i32> {
        self.try_get(row, column)
            .and_then(|x| x.parse::<i32>().ok())
    }

    pub fn get_int(&self, row: usize, column: usize) -> i32 {
        self.try_get(row, column)
            .unwrap_or("")
            .parse::<i32>()
            .unwrap_or(0)
    }
}

#[macro_export]
macro_rules! stb_column {
    (
        $column_index:literal, $name:ident, &str
    ) => {
        pub fn $name(&self, row: usize) -> Option<&str> {
            self.0.try_get(row, $column_index)
        }
    };
    (
        $column_index:literal, $name:ident, bool
    ) => {
        pub fn $name(&self, row: usize) -> Option<bool> {
            self.0
                .try_get(row, $column_index)
                .and_then(|x| x.parse::<i32>().ok())
                .map(|x| x != 0)
        }
    };
    (
        $column_index:literal, $name:ident, $value_type:ty
    ) => {
        pub fn $name(&self, row: usize) -> Option<$value_type> {
            self.0
                .try_get(row, $column_index)
                .and_then(|x| x.parse::<$value_type>().ok())
        }
    };
    (
        $range:expr, $name:ident, ArrayVec< $value_type:ty, $len:literal >
    ) => {
        pub fn $name(&self, row: usize) -> ArrayVec<$value_type, $len> {
            let mut result: ArrayVec<$value_type, $len> = ArrayVec::new();

            for column in $range {
                if let Some(value) = self
                    .0
                    .try_get(row, column)
                    .and_then(|x| x.parse::<$value_type>().ok())
                {
                    result.push(value);
                }
            }

            result
        }
    };
    (
        $range:expr, $name:ident, [Option<$value_type:ty>; $len:literal]
    ) => {
        pub fn $name(&self, row: usize) -> [Option<$value_type>; $len] {
            let mut result: [Option<$value_type>; $len] = Default::default();

            for (i, column) in ($range).enumerate() {
                if let Some(value) = self
                    .0
                    .try_get(row, column)
                    .and_then(|x| x.parse::<$value_type>().ok())
                {
                    result[i] = Some(value);
                }
            }

            result
        }
    };
    (
        $range:expr, $name:ident, [$value_type:ty; $len:literal]
    ) => {
        pub fn $name(&self, row: usize) -> [$value_type; $len] {
            let mut result: [$value_type; $len] = Default::default();

            for (i, column) in ($range).enumerate() {
                result[i] = self
                    .0
                    .try_get(row, column)
                    .and_then(|x| x.parse::<$value_type>().ok())
                    .unwrap_or(0);
            }

            result
        }
    };
}

#[cfg(test)]
mod tests {
    use super::StbFile;
    use crate::{RoseFile, RoseFileWriter, StbReadOptions};

    fn make_test_stb() -> StbFile {
        let bytes: &[u8] = &[
            b'S', b'T', b'B', b'1', 0x27, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00,
            0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00,
            0x00, 0x00, 0x01, 0x00, b'R', 0x02, 0x00, b'C', b'1', 0x02, 0x00, b'C', b'2', 0x02,
            0x00, b'C', b'3', 0x01, 0x00, b'#', 0x02, 0x00, b'R', b'1', 0x02, 0x00, b'R', b'2',
            0x02, 0x00, b'A', b'1', 0x02, 0x00, b'B', b'1', 0x00, 0x00, 0x02, 0x00, b'A', b'2',
            0x00, 0x00, 0x02, 0x00, b'C', b'2',
        ];
        StbFile::read(
            bytes.into(),
            &StbReadOptions {
                with_keys: true,
                is_wide: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn stb_round_trip_preserves_logical_content() {
        let stb = make_test_stb();
        let mut writer = RoseFileWriter::default();
        stb.write(&mut writer, &()).unwrap();

        let reread = StbFile::read(
            writer.buffer.as_ref().into(),
            &StbReadOptions {
                with_keys: true,
                is_wide: false,
            },
        )
        .unwrap();

        assert_eq!(reread.version(), 1);
        assert_eq!(reread.rows(), 2);
        assert_eq!(reread.columns(), 3);
        assert_eq!(reread.get_row_name(0), "R1");
        assert_eq!(reread.get_row_name(1), "R2");
        assert_eq!(reread.get(0, 0), "A1");
        assert_eq!(reread.get(0, 1), "B1");
        assert_eq!(reread.get(0, 2), "");
        assert_eq!(reread.get(1, 0), "A2");
        assert_eq!(reread.get(1, 1), "");
        assert_eq!(reread.get(1, 2), "C2");
        assert_eq!(reread.lookup_row_name("R2"), Some(1));
    }

    #[test]
    fn stb_supports_row_and_cell_mutation() {
        let mut stb = make_test_stb();
        stb.set(0, 2, "NEW").unwrap();
        stb.push_row(
            "SHOP_CLONE_3",
            vec!["10".to_string(), "tab_key".to_string(), "2001".to_string()],
        )
        .unwrap();

        assert_eq!(stb.get(0, 2), "NEW");
        assert_eq!(stb.rows(), 3);
        assert_eq!(stb.get_row_name(2), "SHOP_CLONE_3");
        assert_eq!(stb.get(2, 1), "tab_key");
    }
}
