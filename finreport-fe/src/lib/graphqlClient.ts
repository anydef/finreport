import {createClient, cacheExchange, fetchExchange} from '@urql/core';

export const createGraphqlClient = (fetch: typeof globalThis.fetch) =>
    createClient({
        url: 'http://localhost:8080/graphql',
        exchanges: [cacheExchange, fetchExchange],
        fetch
    });