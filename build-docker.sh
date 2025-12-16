#!/bin/bash
set -e

echo "🐳 Building Docker Images for gRPC GraphQL Gateway"
echo "=================================================="

# Parse arguments
VERSION=${1:-latest}
REGISTRY=${2:-ghcr.io/protocol-lattice}

echo "Version: $VERSION"
echo "Registry: $REGISTRY"
echo ""

# Build main image
echo "📦 Building main gateway image..."
docker build -t ${REGISTRY}/grpc-graphql-gateway:${VERSION} \
  -t ${REGISTRY}/grpc-graphql-gateway:latest \
  -f Dockerfile .

echo "✅ Main image built successfully"
echo ""

# Build federation image
echo "📦 Building federation image..."
docker build -t ${REGISTRY}/grpc-graphql-gateway-federation:${VERSION} \
  -t ${REGISTRY}/grpc-graphql-gateway-federation:latest \
  -f Dockerfile.federation .

echo "✅ Federation image built successfully"
echo ""

# List images
echo "📋 Built images:"
docker images | grep grpc-graphql-gateway

echo ""
echo "🎯 To push images to registry:"
echo "  docker push ${REGISTRY}/grpc-graphql-gateway:${VERSION}"
echo "  docker push ${REGISTRY}/grpc-graphql-gateway:latest"
echo "  docker push ${REGISTRY}/grpc-graphql-gateway-federation:${VERSION}"
echo "  docker push ${REGISTRY}/grpc-graphql-gateway-federation:latest"
echo ""
echo "🧪 To test locally:"
echo "  docker run -p 8080:8080 ${REGISTRY}/grpc-graphql-gateway:${VERSION}"
echo "  docker-compose -f docker-compose.federation.yml up"
