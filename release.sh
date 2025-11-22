#!/bin/bash
# Release script for Sheepnet
# Usage: ./release.sh [patch|minor|major]

set -e

GRADLE_FILE="app/build.gradle.kts"

# Get current version
CURRENT_VERSION=$(grep "versionName = " "$GRADLE_FILE" | sed 's/.*"\(.*\)".*/\1/')
CURRENT_CODE=$(grep "versionCode = " "$GRADLE_FILE" | sed 's/.*= \([0-9]*\).*/\1/')

echo "Current version: $CURRENT_VERSION (code: $CURRENT_CODE)"

# Parse version
IFS='.' read -r -a VERSION_PARTS <<< "$CURRENT_VERSION"
MAJOR="${VERSION_PARTS[0]}"
MINOR="${VERSION_PARTS[1]}"
PATCH="${VERSION_PARTS[2]}"

# Determine new version based on argument
case "$1" in
    patch)
        PATCH=$((PATCH + 1))
        ;;
    minor)
        MINOR=$((MINOR + 1))
        PATCH=0
        ;;
    major)
        MAJOR=$((MAJOR + 1))
        MINOR=0
        PATCH=0
        ;;
    *)
        echo "Usage: $0 [patch|minor|major]"
        echo ""
        echo "Current version: $CURRENT_VERSION"
        echo "  patch: $MAJOR.$MINOR.$((PATCH + 1))"
        echo "  minor: $MAJOR.$((MINOR + 1)).0"
        echo "  major: $((MAJOR + 1)).0.0"
        exit 1
        ;;
esac

NEW_VERSION="$MAJOR.$MINOR.$PATCH"
NEW_CODE=$((CURRENT_CODE + 1))

echo "New version: $NEW_VERSION (code: $NEW_CODE)"
echo ""

# Confirm
read -p "Create release $NEW_VERSION? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 1
fi

# Update build.gradle.kts
sed -i "s/versionCode = $CURRENT_CODE/versionCode = $NEW_CODE/" "$GRADLE_FILE"
sed -i "s/versionName = \"$CURRENT_VERSION\"/versionName = \"$NEW_VERSION\"/" "$GRADLE_FILE"

echo "✓ Updated $GRADLE_FILE"

# Git commit and tag
git add "$GRADLE_FILE"
git commit -m "Release v$NEW_VERSION"
git tag "v$NEW_VERSION"

echo "✓ Created commit and tag v$NEW_VERSION"
echo ""
echo "Next steps:"
echo "  git push origin main"
echo "  git push origin v$NEW_VERSION"
echo ""
echo "Or in one command:"
echo "  git push origin main && git push origin v$NEW_VERSION"

