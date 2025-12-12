# File Uploads

The gateway automatically supports GraphQL file uploads via multipart requests, following the [GraphQL multipart request specification](https://github.com/jaydenseric/graphql-multipart-request-spec).

## Proto Definition

Map `bytes` fields to handle file uploads:

```protobuf
message UploadAvatarRequest {
  string user_id = 1;
  bytes avatar = 2;  // Maps to Upload scalar in GraphQL
}

message UploadAvatarResponse {
  string user_id = 1;
  int64 size = 2;
}

service UserService {
  rpc UploadAvatar(UploadAvatarRequest) returns (UploadAvatarResponse) {
    option (graphql.schema) = {
      type: MUTATION
      name: "uploadAvatar"
      request { name: "input" }
    };
  }
}
```

## GraphQL Mutation

The generated GraphQL schema includes an `Upload` scalar:

```graphql
mutation UploadAvatar($file: Upload!) {
  uploadAvatar(input: { userId: "123", avatar: $file }) {
    userId
    size
  }
}
```

## Using curl

```bash
curl http://localhost:8888/graphql \
  --form 'operations={"query": "mutation($file: Upload!) { uploadAvatar(input:{userId:\"123\", avatar:$file}) { userId size } }", "variables": {"file": null}}' \
  --form 'map={"0": ["variables.file"]}' \
  --form '0=@avatar.png'
```

### Request Format

1. **operations** - JSON containing the query and variables
2. **map** - Maps file indices to variable paths
3. **0, 1, ...** - The actual file content

## JavaScript Client

Using Apollo Client with `apollo-upload-client`:

```javascript
import { createUploadLink } from 'apollo-upload-client';
import { ApolloClient, InMemoryCache } from '@apollo/client';

const client = new ApolloClient({
  link: createUploadLink({ uri: 'http://localhost:8888/graphql' }),
  cache: new InMemoryCache(),
});

// Upload mutation
const UPLOAD_AVATAR = gql`
  mutation UploadAvatar($file: Upload!) {
    uploadAvatar(input: { userId: "123", avatar: $file }) {
      userId
      size
    }
  }
`;

// Trigger upload
const file = document.querySelector('input[type="file"]').files[0];
client.mutate({
  mutation: UPLOAD_AVATAR,
  variables: { file },
});
```

## Multiple Files

Upload multiple files by adding more entries to the map:

```bash
curl http://localhost:8888/graphql \
  --form 'operations={"query": "mutation($files: [Upload!]!) { uploadFiles(files: $files) { count } }", "variables": {"files": [null, null]}}' \
  --form 'map={"0": ["variables.files.0"], "1": ["variables.files.1"]}' \
  --form '0=@file1.pdf' \
  --form '1=@file2.pdf'
```

## File Size Limits

By default, uploads are limited by your web server configuration. For large files, consider:

1. Streaming uploads to avoid memory pressure
2. Setting appropriate timeouts
3. Using a CDN or object storage for very large files

## Backend Handling

On the gRPC backend, the file is received as `bytes`. Example in Rust:

```rust
async fn upload_avatar(
    &self,
    request: Request<UploadAvatarRequest>,
) -> Result<Response<UploadAvatarResponse>, Status> {
    let req = request.into_inner();
    let file_data = req.avatar;  // Vec<u8>
    let size = file_data.len() as i64;
    
    // Save file, upload to S3, etc.
    
    Ok(Response::new(UploadAvatarResponse {
        user_id: req.user_id,
        size,
    }))
}
```
