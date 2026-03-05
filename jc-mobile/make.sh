#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"

SCHEME="jc-mobile"
PROJECT="jc-mobile.xcodeproj"
BUILD_DIR="build"
ENV_FILE=".env"

# ---------- colors ----------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

err()  { echo -e "${RED}ERROR:${NC} $*" >&2; }
ok()   { echo -e "${GREEN}OK:${NC} $*"; }
warn() { echo -e "${YELLOW}NOTE:${NC} $*"; }

# ---------- preflight checks ----------

check_xcode() {
    if ! xcode-select -p &>/dev/null; then
        err "Xcode command line tools not found."
        echo ""
        echo "  Install with:"
        echo "    xcode-select --install"
        echo ""
        exit 1
    fi

    if ! xcrun --sdk iphoneos --show-sdk-path &>/dev/null; then
        err "iOS SDK not found. You need full Xcode (not just CLI tools)."
        echo ""
        echo "  1. Install Xcode from the App Store"
        echo "  2. Run: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"
        echo ""
        exit 1
    fi
    ok "Xcode toolchain found"
}

load_team() {
    if [ ! -f "$ENV_FILE" ]; then
        err "No .env file found. Development team ID is required for code signing."
        echo ""
        echo "  To find your team ID, check an existing provisioning profile:"
        echo "    security cms -D -i ~/Library/Developer/Xcode/UserData/Provisioning\\ Profiles/*.mobileprovision 2>/dev/null | grep -A1 TeamIdentifier"
        echo ""
        echo "  Or look at the app ID prefix (the part before the dot):"
        echo "    security cms -D -i ~/Library/Developer/Xcode/UserData/Provisioning\\ Profiles/*.mobileprovision 2>/dev/null | grep application-identifier"
        echo "    => <string>ABCDE12345.com.example.app</string>"
        echo "               ^^^^^^^^^^"
        echo ""
        echo "  Then create ${ENV_FILE} with:"
        echo "    echo 'DEVELOPMENT_TEAM=ABCDE12345' > ${ENV_FILE}"
        echo ""
        exit 1
    fi

    # shellcheck source=/dev/null
    source "$ENV_FILE"

    if [ -z "${DEVELOPMENT_TEAM:-}" ]; then
        err ".env file exists but DEVELOPMENT_TEAM is not set."
        echo ""
        echo "  Add this line to ${ENV_FILE}:"
        echo "    DEVELOPMENT_TEAM=YOUR_TEAM_ID"
        echo ""
        exit 1
    fi
    ok "Development team: ${DEVELOPMENT_TEAM}"
}

find_device() {
    local json_file="/tmp/jc-devices-$$.json"
    xcrun devicectl list devices -j "$json_file" >/dev/null 2>&1 || { rm -f "$json_file"; return 1; }

    # Extract first available iPhone identifier
    DEVICE_UDID=$(python3 - "$json_file" <<'PYEOF'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
    devices = data.get('result', {}).get('devices', [])
    for d in devices:
        model = d.get('hardwareProperties', {}).get('productType', '')
        if not model.startswith('iPhone'):
            continue
        print(d['identifier'])
        sys.exit(0)
except Exception as e:
    print(str(e), file=sys.stderr)
sys.exit(1)
PYEOF
    ) || { rm -f "$json_file"; return 1; }
    rm -f "$json_file"
}

check_device() {
    if ! find_device; then
        err "No connected iPhone found."
        echo ""
        echo "  1. Connect your iPhone via USB"
        echo "  2. Trust this computer on the phone if prompted"
        echo "  3. Verify with: xcrun devicectl list devices"
        echo ""
        exit 1
    fi
    ok "Found device: ${DEVICE_UDID}"
}

# ---------- actions ----------

do_build() {
    check_xcode
    load_team

    echo ""
    echo -e "${BOLD}Building ${SCHEME}...${NC}"
    local build_log
    build_log=$(mktemp)
    local build_rc=0
    xcodebuild \
        -project "$PROJECT" \
        -scheme "$SCHEME" \
        -configuration Debug \
        -destination "generic/platform=iOS" \
        -derivedDataPath "$BUILD_DIR" \
        -allowProvisioningUpdates \
        DEVELOPMENT_TEAM="$DEVELOPMENT_TEAM" \
        CODE_SIGN_STYLE=Automatic \
        build 2>&1 | tee "$build_log" | tail -5 || build_rc=$?

    if [ "$build_rc" -ne 0 ]; then
        echo ""
        if grep -q "No Account for Team" "$build_log"; then
            err "Xcode has no Apple ID linked to team ${DEVELOPMENT_TEAM}."
            echo ""
            echo "  One-time fix (you won't need Xcode again after this):"
            echo ""
            echo "  1. open jc-mobile.xcodeproj"
            echo "  2. Xcode > Settings > Accounts — verify your Apple ID is listed"
            echo "  3. Select the jc-mobile target > Signing & Capabilities"
            echo "  4. Pick your team from the dropdown"
            echo "  5. Build once (Cmd-B) to create the provisioning profile"
            echo "  6. Close Xcode. CLI builds will work from now on."
            echo ""
        elif grep -q "No profiles for" "$build_log"; then
            err "No provisioning profile for this bundle ID."
            echo ""
            echo "  One-time fix:"
            echo ""
            echo "  1. open jc-mobile.xcodeproj"
            echo "  2. Select the jc-mobile target > Signing & Capabilities"
            echo "  3. Ensure 'Automatically manage signing' is checked"
            echo "  4. Select your team"
            echo "  5. Build once (Cmd-B)"
            echo "  6. Close Xcode. CLI builds will work from now on."
            echo ""
        elif grep -q "requires a provisioning profile" "$build_log" || grep -q "doesn't support the App Groups" "$build_log"; then
            err "Provisioning issue. See above for details."
            echo ""
            echo "  Try: open jc-mobile.xcodeproj, fix signing, build once, close Xcode."
            echo ""
        else
            err "Build failed. Last 30 lines of log:"
            tail -30 "$build_log"
        fi
        rm -f "$build_log"
        exit 1
    fi
    rm -f "$build_log"

    APP_PATH=$(find "$BUILD_DIR" -name "*.app" -path "*/Debug-iphoneos/*" -maxdepth 6 | head -1)
    if [ -z "$APP_PATH" ]; then
        err "Build succeeded but .app not found in ${BUILD_DIR}"
        exit 1
    fi
    ok "Built: ${APP_PATH}"
}

do_deploy() {
    do_build
    check_device

    echo ""
    echo -e "${BOLD}Installing on device...${NC}"
    xcrun devicectl device install app --device "$DEVICE_UDID" "$APP_PATH"

    BUNDLE_ID=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$APP_PATH/Info.plist")
    echo ""
    echo -e "${BOLD}Launching ${BUNDLE_ID}...${NC}"
    xcrun devicectl device process launch --device "$DEVICE_UDID" "$BUNDLE_ID" || warn "Launch failed (app may still have been installed)"

    ok "Deployed to device"
    echo ""
    echo "  To view logs:"
    echo "    xcrun devicectl device process launch --console --device $DEVICE_UDID $BUNDLE_ID"
}

do_clean() {
    rm -rf "$BUILD_DIR"
    ok "Cleaned build directory"
}

# ---------- main ----------

case "${1:-build}" in
    build)  do_build ;;
    deploy) do_deploy ;;
    clean)  do_clean ;;
    *)
        echo "Usage: $0 [build|deploy|clean]"
        echo ""
        echo "  build   Build for iOS device (default)"
        echo "  deploy  Build and install on connected iPhone"
        echo "  clean   Remove build artifacts"
        exit 1
        ;;
esac
