//! OpenAPI document, assembled with utoipa's builder API.
//!
//! The handlers are generic over `Entity`, so `#[utoipa::path]` cannot be
//! applied to them. Instead the document is generated once, in a loop over
//! `ALL_PATHS`, and served at `/openapi.json` by the Swagger UI router.

use utoipa::openapi::{
    path::{HttpMethod, OperationBuilder, ParameterBuilder, ParameterIn, PathItemBuilder},
    request_body::RequestBodyBuilder,
    schema::{ObjectBuilder, Type},
    security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme},
    tag::TagBuilder,
    ComponentsBuilder, ContentBuilder, InfoBuilder, OpenApi, OpenApiBuilder, PathsBuilder, Ref, Required,
    ResponseBuilder,
};

use crate::api::dto::{PageResponse, RecordRequest, RecordResponse};
use crate::api::entity::ALL_PATHS;
use crate::domain::FieldError;
use crate::error::ProblemDetails;

const BEARER: &str = "bearerAuth";

fn json_response(desc: &str, schema: &str) -> utoipa::openapi::Response {
    ResponseBuilder::new()
        .description(desc)
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(Ref::from_schema_name(schema)))
                .build(),
        )
        .build()
}

fn problem(desc: &str) -> utoipa::openapi::Response {
    ResponseBuilder::new()
        .description(desc)
        .content(
            "application/problem+json",
            ContentBuilder::new()
                .schema(Some(Ref::from_schema_name("ProblemDetails")))
                .build(),
        )
        .build()
}

fn id_param() -> utoipa::openapi::path::Parameter {
    ParameterBuilder::new()
        .name("id")
        .parameter_in(ParameterIn::Path)
        .required(Required::True)
        .schema(Some(ObjectBuilder::new().schema_type(Type::Integer).build()))
        .build()
}

fn query_param(name: &str, ty: Type, desc: &str) -> utoipa::openapi::path::Parameter {
    ParameterBuilder::new()
        .name(name)
        .parameter_in(ParameterIn::Query)
        .required(Required::False)
        .description(Some(desc))
        .schema(Some(ObjectBuilder::new().schema_type(ty).build()))
        .build()
}

fn body() -> utoipa::openapi::request_body::RequestBody {
    RequestBodyBuilder::new()
        .required(Some(Required::True))
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(Ref::from_schema_name("RecordRequest")))
                .build(),
        )
        .build()
}

fn secured(op: OperationBuilder) -> OperationBuilder {
    op.security(SecurityRequirement::new(BEARER, Vec::<String>::new()))
        .response("401", problem("Missing or invalid token"))
        .response("403", problem("Token lacks write scope"))
}

pub fn document() -> OpenApi {
    let mut paths = PathsBuilder::new();
    let mut tags = Vec::new();

    for (_, path, tag) in ALL_PATHS {
        tags.push(TagBuilder::new().name(tag).build());
        let collection = format!("/api/v1/{path}");
        let item = format!("/api/v1/{path}/{{id}}");

        let list = OperationBuilder::new()
            .tag(tag)
            .summary(Some(format!("List {tag}")))
            .parameter(query_param("code", Type::String, "exact code match"))
            .parameter(query_param("name", Type::String, "case-insensitive substring"))
            .parameter(query_param("limit", Type::Integer, "1..=500, default 50"))
            .parameter(query_param("offset", Type::Integer, ">= 0, default 0"))
            .response("200", json_response("Page of records", "PageResponse"))
            .response("408", problem("Request exceeded the server timeout"))
            .response("422", problem("Invalid query parameters"));

        let create = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Create {tag}")))
                .request_body(Some(body()))
                .response("201", json_response("Created", "RecordResponse"))
                .response("405", problem("Method not allowed"))
                .response("408", problem("Request exceeded the server timeout"))
                .response("409", problem("Duplicate code"))
                .response("422", problem("Validation failed")),
        );

        let get_one = OperationBuilder::new()
            .tag(tag)
            .summary(Some(format!("Get one of {tag}")))
            .parameter(id_param())
            .response("200", json_response("Record", "RecordResponse"))
            .response("404", problem("Not found"))
            .response("408", problem("Request exceeded the server timeout"));

        let if_match = ParameterBuilder::new()
            .name("If-Match")
            .parameter_in(ParameterIn::Header)
            .required(Required::True)
            .description(Some("Quoted current version, e.g. \"3\""))
            .schema(Some(ObjectBuilder::new().schema_type(Type::String).build()))
            .build();

        let update = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Replace one of {tag}")))
                .parameter(id_param())
                .parameter(if_match)
                .request_body(Some(body()))
                .response("200", json_response("Updated", "RecordResponse"))
                .response("404", problem("Not found"))
                .response("405", problem("Method not allowed"))
                .response("408", problem("Request exceeded the server timeout"))
                .response("409", problem("Version mismatch or duplicate code"))
                .response("422", problem("Validation failed"))
                .response("428", problem("If-Match missing or malformed")),
        );

        let delete = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Soft-delete one of {tag}")))
                .parameter(id_param())
                .response("405", problem("Method not allowed"))
                .response("408", problem("Request exceeded the server timeout"))
                .response(
                    "204",
                    ResponseBuilder::new().description("Deleted (idempotent)").build(),
                ),
        );

        paths = paths
            .path(
                collection,
                PathItemBuilder::new()
                    .operation(HttpMethod::Get, list.build())
                    .operation(HttpMethod::Post, create.build())
                    .build(),
            )
            .path(
                item,
                PathItemBuilder::new()
                    .operation(HttpMethod::Get, get_one.build())
                    .operation(HttpMethod::Put, update.build())
                    .operation(HttpMethod::Delete, delete.build())
                    .build(),
            );
    }

    let components = ComponentsBuilder::new()
        .schema_from::<RecordRequest>()
        .schema_from::<RecordResponse>()
        .schema_from::<PageResponse>()
        .schema_from::<ProblemDetails>()
        .schema_from::<FieldError>()
        .security_scheme(
            BEARER,
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        )
        .build();

    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("Master Data API")
                .version(env!("CARGO_PKG_VERSION"))
                .build(),
        )
        .paths(paths.build())
        .components(Some(components))
        .tags(Some(tags))
        .build()
}
