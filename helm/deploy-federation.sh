#!/bin/bash
set -e

echo "🚀 Deploying Federation Example with Helm"
echo "=========================================="

# Create namespace
echo "📦 Creating namespace..."
kubectl create namespace federation-example --dry-run=client -o yaml | kubectl apply -f -

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
  --set persistence.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=user \
  --set env[1].name=PORT \
  --set env[1].value="8891" \
  --set env[2].name=GRPC_PORT \
  --set env[2].value="50051" \
  --set env[3].name=METRICS_PORT \
  --set env[3].value="9090"


# Deploy Product Subgraph
echo ""
echo "🛍️ Deploying Product Subgraph..."
helm upgrade --install product-subgraph ./grpc-graphql-gateway \
  --namespace federation-example \
  --set nameOverride=product-subgraph \
  --set fullnameOverride=product-subgraph \
  --set service.httpPort=8892 \
  --set service.grpcPort=50052 \
  --set ingress.enabled=false \
  --set persistence.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=product \
  --set env[1].name=PORT \
  --set env[1].value="8892" \
  --set env[2].name=GRPC_PORT \
  --set env[2].value="50052" \
  --set env[3].name=METRICS_PORT \
  --set env[3].value="9090"


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
  --set persistence.enabled=false \
  --set env[0].name=SERVICE_NAME \
  --set env[0].value=review \
  --set env[1].name=PORT \
  --set env[1].value="8893" \
  --set env[2].name=GRPC_PORT \
  --set env[2].value="50053" \
  --set env[3].name=METRICS_PORT \
  --set env[3].value="9090"


# Deploy Router
echo ""
echo "🌐 Deploying Apollo Router..."
echo "Generating K8s Supergraph..."
# Assuming rover is installed and running from root context
if [ -f "../examples/federation/supergraph-k8s.yaml" ]; then
    rover supergraph compose --config ../examples/federation/supergraph-k8s.yaml > ../examples/federation/supergraph-k8s.graphql
fi

kubectl create configmap router-schema \
  --namespace federation-example \
  --from-file=supergraph.graphql=../examples/federation/supergraph-k8s.graphql \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl create configmap router-config \
  --namespace federation-example \
  --from-file=router.yaml=../examples/federation/router-config.yaml \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl apply -f router.yaml

echo ""
echo "✅ Deployment complete!"
echo ""
echo "To access the subgraphs locally:"
echo "kubectl port-forward -n federation-example svc/user-subgraph 8891:8891 &"
echo "kubectl port-forward -n federation-example svc/product-subgraph 8892:8892 &"
echo "kubectl port-forward -n federation-example svc/review-subgraph 8893:8893 &"
echo ""
echo "To access the Router:"
echo "kubectl port-forward -n federation-example svc/router 4000:4000"
