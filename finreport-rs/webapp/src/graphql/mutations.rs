use async_graphql::Object;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn load_accounts(&self) -> Result<bool, async_graphql::Error> {
        unimplemented!(
            "Has to trigger the loading of accounts \
        from the comdirect API and store them in the database"
        );
    }
}
