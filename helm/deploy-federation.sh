#!/bin/bash
set -e

echo "🚀 Deploying Federation Example with Helm"
echo "=========================================="

# Create namespace
echo "📦 Creating namespace..."
kubectl create namespace federation-example --dry-run=client -o yaml | kubectl apply -f -

# Check if Helm chart exists
if [ ! -d "./grpc-graphql-gateway" ]; then
    echo "❌ Error: Helm chart not found in current directory"
    echo "   Please run this script from the helm/ directory"
    exit 1
fi

# Deploy User Subgraph
echo ""
echo "👤 Deploying User Subgraph..."
helm upgrade --install user-subgraph ./grpc-graphql-gateway \
  --namespace federation-example \
  --set nameOverride=user-subgraph \
  --set fullnameOverride=user-subgraph \
  --set service.httpPort=8891 \
  --set service.grpcPort=50051 \
  --set ingress.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=user \
  --set env[1].name=PORT \
  --set env[1].value="8891" \
  --wait

# Deploy Product Subgraph
echo ""
echo "📦 Deploying Product Subgraph..."
helm upgrade --install product-subgraph ./grpc-graphql-gateway \
  --namespace federation-example \
  --set nameOverride=product-subgraph \
  --set fullnameOverride=product-subgraph \
  --set service.httpPort=8892 \
  --set service.grpcPort=50052 \
  --set ingress.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=product \
  --set env[1].name=PORT \
  --set env[1].value="8892" \
  --wait

# Deploy Review Subgraph
echo ""
echo "⭐ Deploying Review Subgraph..."
helm upgrade --install review-subgraph ./grpc-graphql-gateway \
  --namespace federation-example \
  --set nameOverride=review-subgraph \
  --set fullnameOverride=review-subgraph \
  --set service.httpPort=8893 \
  --set service.grpcPort=50053 \
  --set ingress.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=review \
  --set env[1].name=PORT \
  --set env[1].value="8893" \
  --wait

echo ""
echo "✅ Federation deployment complete!"
echo ""
echo "📊 Check deployment status:"
echo "  kubectl get pods -n federation-example"
echo "  kubectl get svc -n federation-example"
echo ""
echo "🔗 Port forward to test locally:"
echo "  kubectl port-forward -n federation-example svc/user-subgraph 8891:8891"
echo "  kubectl port-forward -n federation-example svc/product-subgraph 8892:8892"
echo "  kubectl port-forward -n federation-example svc/review-subgraph 8893:8893"
echo ""
echo "🧪 Test queries:"
echo '  curl -X POST http://localhost:8891/graphql -H "Content-Type: application/json" -d '"'"'{"query": "{ user(id:\"u1\") { id name } }"}'"'"
echo ""
echo "🗑️  To cleanup:"
echo "  helm uninstall user-subgraph product-subgraph review-subgraph -n federation-example"
echo "  kubectl delete namespace federation-example"
