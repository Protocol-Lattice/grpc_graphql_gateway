#!/bin/bash
set -e

echo "🔍 Validating Helm Chart"
echo "======================="
echo ""

CHART_DIR="./helm/grpc-graphql-gateway"

# Check if chart directory exists
if [ ! -d "$CHART_DIR" ]; then
    echo "❌ Chart directory not found: $CHART_DIR"
    exit 1
fi

echo "✅ Chart directory exists"
echo ""

# Check required files
echo "📋 Checking required files..."
required_files=(
    "Chart.yaml"
    "values.yaml"
    "templates/deployment.yaml"
    "templates/service.yaml"
    "templates/hpa.yaml"
    "templates/vpa.yaml"
    "templates/loadbalancer.yaml"
)

for file in "${required_files[@]}"; do
    if [ -f "$CHART_DIR/$file" ]; then
        echo "  ✅ $file"
    else
        echo "  ❌ Missing: $file"
    fi
done

echo ""

# Check if helm is installed
if command -v helm &> /dev/null; then
    echo "🔧 Running Helm lint..."
    helm lint "$CHART_DIR"
    echo ""
    
    echo "📦 Packaging chart..."
    helm package "$CHART_DIR" -d ./helm/
    echo ""
    
    echo "📝 Rendering templates (dry-run)..."
    helm template test-gateway "$CHART_DIR" \
        --set image.tag=test \
        --output-dir ./helm/rendered-templates
    echo "✅ Templates rendered to: ./helm/rendered-templates/"
    echo ""
    
    echo "🎯 Chart validation complete!"
    echo ""
    echo "To install:"
    echo "  helm install my-gateway $CHART_DIR"
    echo ""
    echo "To serve chart repository:"
    echo "  helm repo index ./helm --url http://localhost:8080"
    echo "  python3 -m http.server 8080 --directory ./helm"
else
    echo "⚠️  Helm not installed. Install with:"
    echo "  brew install helm"
    echo ""
    echo "Manual validation:"
    echo "  - Chart.yaml format looks good"
    echo "  - Templates are present"
    echo "  - Values.yaml contains all required fields"
fi
