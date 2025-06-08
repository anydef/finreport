
import { createGraphqlClient } from '$lib/graphqlClient';

const QUERY = `
query ReportQuery {
    reports(month: "10", year: "2025") {
        __typename,
        month,
        year,
        category,
        totalIncome,
        totalExpenses
    }
}
`;


export async function load({fetch, params}) {
  const client = createGraphqlClient(fetch);
  const result = await client.query(QUERY, {}).toPromise();
  if (result.error) {
    return { error: 'Failed to fetch data from GraphQL API' };
  }
  return { payload: result.data.reports };
}


// export async function load({ fetch }){
//     const response = await fetch('http://localhost:8080/test-chart');
//     if (response.ok) {
//         const data = await response.json();
//         return {payload: data};
//     } else {
//         return { error: 'Failed to fetch data from external server' };
//     }
// };