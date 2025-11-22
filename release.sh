#!/bin/bash
set -e

# Sheepnet Release Script
# Bumps version, creates git tag, and triggers automated release build

BUMP_TYPE="$1"

if [ -z "$BUMP_TYPE" ]; then
    echo "Usage: $0 <patch|minor|major>"
    echo ""
    echo "  patch  - 0.1.0 -> 0.1.1 (bug fixes)"
    echo "  minor  - 0.1.0 -> 0.2.0 (new features)"
    echo "  major  - 0.1.0 -> 1.0.0 (breaking changes)"
    exit 1
fi

if [[ ! "$BUMP_TYPE" =~ ^(patch|minor|major)$ ]]; then
    echo "Error: Bump type must be 'patch', 'minor', or 'major'"
    exit 1
fi

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

if [ -z "$CURRENT_VERSION" ]; then
    echo "Error: Could not find version in Cargo.toml"
    exit 1
fi

# Parse version
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Bump version based on type
case "$BUMP_TYPE" in
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
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"

echo "Bumping version: ${CURRENT_VERSION} -> ${NEW_VERSION} (${BUMP_TYPE})"

# Check for uncommitted changes
if [ -n "$(git status --porcelain)" ]; then
    echo "Error: You have uncommitted changes. Please commit or stash them first."
    git status --short
    exit 1
fi

# Check if tag already exists
if git rev-parse "v${NEW_VERSION}" >/dev/null 2>&1; then
    echo "Error: Tag v${NEW_VERSION} already exists"
    exit 1
fi

# Update version in Cargo.toml
sed -i "s/^version = \".*\"/version = \"${NEW_VERSION}\"/" Cargo.toml

# Update Cargo.lock
cargo check --quiet 2>/dev/null

# Commit changes
git add Cargo.toml Cargo.lock
git commit -m "Release v${NEW_VERSION}" --quiet

# Create tag
git tag -a "v${NEW_VERSION}" -m "Release v${NEW_VERSION}"

echo "Created tag v${NEW_VERSION}"
echo ""
echo "Push with: git push && git push origin v${NEW_VERSION}"
