use graphql_client::GraphQLQuery;

type GitObjectID = String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/pull_request.graphql",
    response_derives = "Debug"
)]
pub struct PullRequestQuery;

pub fn actor_id(actor: &pull_request_query::ActorProps) -> &str {
    match &actor.on {
        pull_request_query::ActorPropsOn::Bot(x) => &x.id,
        pull_request_query::ActorPropsOn::EnterpriseUserAccount(x) => &x.id,
        pull_request_query::ActorPropsOn::Mannequin(x) => &x.id,
        pull_request_query::ActorPropsOn::Organization(x) => &x.id,
        pull_request_query::ActorPropsOn::User(x) => &x.id,
    }
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/dequeue.graphql",
    response_derives = "Debug"
)]
pub struct DequeuePullRequest;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/enqueue.graphql",
    response_derives = "Debug"
)]
pub struct EnqueuePullRequest;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/disable_automerge.graphql",
    response_derives = "Debug"
)]
pub struct DisableAutomerge;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/enable_automerge.graphql",
    response_derives = "Debug"
)]
pub struct EnableAutomerge;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/merge.graphql",
    response_derives = "Debug"
)]
pub struct MergePullRequest;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/check_permission.graphql",
    response_derives = "Debug"
)]
pub struct CheckPermission;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/add_comment.graphql",
    response_derives = "Debug"
)]
pub struct AddComment;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/pull_request_by_number.graphql",
    response_derives = "Debug"
)]
pub struct PullRequestByNumber;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/approve.graphql",
    response_derives = "Debug"
)]
pub struct Approve;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "queries/schema.graphql",
    query_path = "queries/dismiss_review.graphql",
    response_derives = "Debug"
)]
pub struct DismissReview;
