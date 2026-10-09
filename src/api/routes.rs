use axum::{
    routing::{get, post},
    Router,
};

use crate::api::{
    entity::{
        Age, BusinessUnit, Category, Country, Currency, Entity, Gender, Geography, Language, ProductLine,
        ProductTier,
    },
    handlers, AppState,
};

fn entity_router<E: Entity>() -> Router<AppState> {
    Router::new()
        // The collection is registered at "/", not "": axum rejects an empty path outright
        // ("Paths must start with a `/`"), and `Router::nest` maps an inner path of exactly
        // "/" onto the mount prefix itself. So `nest("/api/v1/{PATH}")` yields the collection
        // at `/api/v1/{PATH}` (and `/{id}` at `/api/v1/{PATH}/{id}`) without a trailing slash.
        .route("/", post(handlers::create::<E>).get(handlers::list::<E>))
        .route(
            "/{id}",
            get(handlers::get_one::<E>)
                .put(handlers::update::<E>)
                .delete(handlers::delete::<E>),
        )
}

fn mount<E: Entity>(router: Router<AppState>) -> Router<AppState> {
    router.nest(&format!("/api/v1/{}", E::PATH), entity_router::<E>())
}

pub fn entity_routes() -> Router<AppState> {
    let r = Router::new();
    let r = mount::<Country>(r);
    let r = mount::<Currency>(r);
    let r = mount::<Language>(r);
    let r = mount::<Category>(r);
    let r = mount::<Gender>(r);
    let r = mount::<Age>(r);
    let r = mount::<Geography>(r);
    let r = mount::<ProductLine>(r);
    let r = mount::<BusinessUnit>(r);
    mount::<ProductTier>(r)
}
