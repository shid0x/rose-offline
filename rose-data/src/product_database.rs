use crate::ItemReference;

/// A single material requirement for a crafting recipe
#[derive(Clone, Debug)]
pub struct ProductMaterial {
    pub item: ItemReference,
    pub quantity: u32,
}

/// A crafting recipe loaded from LIST_PRODUCT.STB
#[derive(Clone, Debug)]
pub struct ProductData {
    /// The raw material class type for the first slot
    pub raw_material_type: u32,
    /// Up to 4 required materials (item reference + quantity)
    pub materials: Vec<ProductMaterial>,
}

/// Database of crafting recipes indexed by product ID
pub struct ProductDatabase {
    products: Vec<Option<ProductData>>,
}

impl ProductDatabase {
    pub fn new(products: Vec<Option<ProductData>>) -> Self {
        Self { products }
    }

    /// Get a product recipe by its product index (from BaseItemData.craft_material)
    pub fn get_product(&self, product_id: u32) -> Option<&ProductData> {
        self.products
            .get(product_id as usize)
            .and_then(|x| x.as_ref())
    }

    pub fn len(&self) -> usize {
        self.products.len()
    }

    pub fn is_empty(&self) -> bool {
        self.products.is_empty()
    }
}
