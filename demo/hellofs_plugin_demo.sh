#!/bin/bash
# HelloFS Plugin Demo - Dynamic plugin loading

SERVER="http://localhost:9999"
PLUGIN_PATH="$(pwd)/target/release/libfs9_plugin_hellofs.so"

echo "=== HelloFS Plugin Demo ==="
echo ""
echo "Plugin path: $PLUGIN_PATH"
echo ""

if [ ! -f "$PLUGIN_PATH" ]; then
    echo "Building hellofs plugin..."
    cargo build -p fs9-plugin-hellofs --release
fi

echo "Loading plugin via API..."
LOAD_RESULT=$(curl -s -X POST "$SERVER/api/v1/plugin/load" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"hellofs\",\"path\":\"$PLUGIN_PATH\"}")
echo "Load result: $LOAD_RESULT"

echo ""
echo "Mounting hellofs at /hello..."
MOUNT_RESULT=$(curl -s -X POST "$SERVER/api/v1/mount" \
    -H "Content-Type: application/json" \
    -d '{"path":"/hello","provider":"hellofs","config":{"greeting":"Welcome to FS9!"}}')
echo "Mount result: $MOUNT_RESULT"

echo ""
echo "Listing /hello directory..."
curl -s "$SERVER/api/v1/readdir?path=/hello" | jq '.'

echo ""
echo "Reading /hello/hello (virtual file)..."
HANDLE=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/hello/hello","flags":{"read":true}}' | jq -r '.handle_id')

if [ "$HANDLE" != "null" ] && [ -n "$HANDLE" ]; then
    curl -s -X POST "$SERVER/api/v1/read" \
        -H "Content-Type: application/json" \
        -d "{\"handle_id\":\"$HANDLE\",\"offset\":0,\"size\":1024}"
    echo ""
    curl -s -X POST "$SERVER/api/v1/close" \
        -H "Content-Type: application/json" \
        -d "{\"handle_id\":\"$HANDLE\",\"sync\":false}" > /dev/null
else
    echo "Failed to open /hello/hello"
fi

echo ""
echo "Creating file /hello/test.txt..."
WRITE_HANDLE=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/hello/test.txt","flags":{"write":true,"create":true}}' | jq -r '.handle_id')

if [ "$WRITE_HANDLE" != "null" ] && [ -n "$WRITE_HANDLE" ]; then
    curl -s -X POST "$SERVER/api/v1/write?handle_id=$WRITE_HANDLE&offset=0" \
        -H "Content-Type: application/octet-stream" \
        -d "Data written via hellofs plugin!" > /dev/null
    curl -s -X POST "$SERVER/api/v1/close" \
        -H "Content-Type: application/json" \
        -d "{\"handle_id\":\"$WRITE_HANDLE\",\"sync\":false}" > /dev/null
    echo "File created!"
fi

echo ""
echo "Listing /hello again..."
curl -s "$SERVER/api/v1/readdir?path=/hello" | jq '.'

echo ""
echo "Reading /hello/test.txt..."
READ_HANDLE=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/hello/test.txt","flags":{"read":true}}' | jq -r '.handle_id')

if [ "$READ_HANDLE" != "null" ] && [ -n "$READ_HANDLE" ]; then
    curl -s -X POST "$SERVER/api/v1/read" \
        -H "Content-Type: application/json" \
        -d "{\"handle_id\":\"$READ_HANDLE\",\"offset\":0,\"size\":1024}"
    echo ""
    curl -s -X POST "$SERVER/api/v1/close" \
        -H "Content-Type: application/json" \
        -d "{\"handle_id\":\"$READ_HANDLE\",\"sync\":false}" > /dev/null
fi

echo ""
echo "=== Demo Complete ==="
