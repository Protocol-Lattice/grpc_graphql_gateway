# Batch Queries

Execute multiple GraphQL operations in a single HTTP request.

## Usage

Send an array of operations:

```bash
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '[
    {"query": "{ users { id name } }"},
    {"query": "{ products { upc price } }"},
    {"query": "mutation { createUser(input: {name: \"Alice\"}) { id } }"}
  ]'
```

## Response Format

Returns an array of responses in the same order:

```json
[
  {"data": {"users": [{"id": "1", "name": "Bob"}]}},
  {"data": {"products": [{"upc": "123", "price": 99}]}},
  {"data": {"createUser": {"id": "2"}}}
]
```

## Benefits

- Reduces HTTP overhead (one connection, one request)
- Atomic execution perception
- Ideal for initial page loads

## Considerations

- Operations execute concurrently (not sequentially)
- Mutations don't wait for previous queries
- Total response size is sum of all responses

## Error Handling

Errors are returned per-operation:

```json
[
  {"data": {"users": [{"id": "1"}]}},
  {"errors": [{"message": "Product not found"}]},
  {"data": {"createUser": {"id": "2"}}}
]
```

## Client Example

```javascript
const batchQuery = async (queries) => {
  const response = await fetch('http://localhost:8888/graphql', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(queries),
  });
  return response.json();
};

const results = await batchQuery([
  { query: '{ users { id } }' },
  { query: '{ products { upc } }' },
]);
```
