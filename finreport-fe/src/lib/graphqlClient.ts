import {createClient, cacheExchange, fetchExchange} from '@urql/core';
import {env} from '$env/dynamic/public';

export const createGraphqlClient = (fetch: typeof globalThis.fetch) =>
    createClient({
        url: env.PUBLIC_GRAPHQL_URL ?? 'http://localhost:8080/graphql',
        exchanges: [cacheExchange, fetchExchange],
        fetch
    });