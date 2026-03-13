use rose_data::{ItemReference, ItemType, ProductData, ProductDatabase, ProductMaterial};
use rose_file_readers::{StbFile, VirtualFilesystem};

use crate::data_decoder::decode_item_base1000;

fn decode_product_material_item(
    material_code: i32,
    row: usize,
    slot: usize,
) -> Option<ItemReference> {
    if material_code <= 0 {
        return None;
    }

    let material_code = material_code as usize;
    if material_code >= 1000 {
        return decode_item_base1000(material_code);
    }

    // Values below 1000 represent class-based raw material requirements in iROSE data.
    // Keep a temporary fallback mapping until class-based requirements are modeled end-to-end.
    log::debug!(
        "LIST_PRODUCT row {} slot {} uses class-coded material {} (<1000); \
         falling back to ItemType::Material for now",
        row,
        slot,
        material_code
    );
    Some(ItemReference::new(ItemType::Material, material_code))
}

pub fn get_product_database(vfs: &VirtualFilesystem) -> Result<ProductDatabase, anyhow::Error> {
    let data = vfs.read_file::<StbFile, _>("3DDATA/STB/LIST_PRODUCT.STB")?;

    let mut products: Vec<Option<ProductData>> = Vec::with_capacity(data.rows());

    for id in 0..data.rows() {
        let raw_material_type = data.try_get_int(id, 1).unwrap_or(0) as u32;
        let mut materials = Vec::new();

        for slot in 0..4 {
            let count_col = 3 + slot * 2;
            let quantity = data.try_get_int(id, count_col).unwrap_or(0) as u32;

            // Match original client slot-0 logic:
            // use raw material code if present, otherwise fall back to NEED_ITEM_NO(..., 0).
            let material_code = if slot == 0 {
                let raw_material_code = data.try_get_int(id, 1).unwrap_or(0);
                if raw_material_code > 0 {
                    raw_material_code
                } else {
                    data.try_get_int(id, 2).unwrap_or(0)
                }
            } else {
                data.try_get_int(id, 2 + slot * 2).unwrap_or(0)
            };

            if quantity != 0 {
                if let Some(item) = decode_product_material_item(material_code, id, slot) {
                    materials.push(ProductMaterial { item, quantity });
                }
            }
        }

        if materials.is_empty() {
            products.push(None);
        } else {
            products.push(Some(ProductData {
                raw_material_type,
                materials,
            }));
        }
    }

    log::debug!(
        "Loaded {} product recipes",
        products.iter().filter(|p| p.is_some()).count()
    );

    Ok(ProductDatabase::new(products))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_product_material_base1000_reference() {
        let item = decode_product_material_item(12041, 0, 0).unwrap();
        assert_eq!(item.item_type, ItemType::Material);
        assert_eq!(item.item_number, 41);
    }

    #[test]
    fn decode_product_material_zero_returns_none() {
        assert!(decode_product_material_item(0, 0, 0).is_none());
    }

    #[test]
    fn decode_product_material_class_code_falls_back_to_material_item_type() {
        let item = decode_product_material_item(421, 0, 0).unwrap();
        assert_eq!(item.item_type, ItemType::Material);
        assert_eq!(item.item_number, 421);
    }
}
