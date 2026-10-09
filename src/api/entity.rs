use crate::domain::EntityType;

/// Compile-time marker binding an entity type to its route path and OpenAPI tag.
pub trait Entity: Send + Sync + 'static {
    const TYPE: EntityType;
    const PATH: &'static str;
    const TAG: &'static str;
}

macro_rules! entities {
    ($( $name:ident => ($ty:expr, $path:literal, $tag:literal) ),* $(,)?) => {
        $(
            pub struct $name;
            impl Entity for $name {
                const TYPE: EntityType = $ty;
                const PATH: &'static str = $path;
                const TAG: &'static str = $tag;
            }
        )*
        /// (entity type, route path, OpenAPI tag) for every entity, for OpenAPI assembly.
        pub const ALL_PATHS: [(EntityType, &str, &str); 10] = [ $( ($ty, $path, $tag) ),* ];
    };
}

entities! {
    Country      => (EntityType::Country,      "countries",      "Countries"),
    Currency     => (EntityType::Currency,     "currencies",     "Currencies"),
    Language     => (EntityType::Language,     "languages",      "Languages"),
    Category     => (EntityType::Category,     "categories",     "Categories"),
    Gender       => (EntityType::Gender,       "genders",        "Genders"),
    Age          => (EntityType::Age,          "ages",           "Ages"),
    Geography    => (EntityType::Geography,    "geographies",    "Geographies"),
    ProductLine  => (EntityType::ProductLine,  "product-lines",  "Product Lines"),
    BusinessUnit => (EntityType::BusinessUnit, "business-units", "Business Units"),
    ProductTier  => (EntityType::ProductTier,  "product-tiers",  "Product Tiers"),
}
