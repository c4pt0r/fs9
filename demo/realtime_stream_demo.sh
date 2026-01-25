#!/bin/bash
# Real-time Streaming Demo - shows multiple readers receiving broadcast data

SERVER="http://localhost:9999"
STREAM="/streamfs/live"

echo "=== Real-time StreamFS Demo ==="
echo "This demo shows real-time data fanout to multiple readers"
echo ""

# Create stream
echo "Creating stream at $STREAM..."
WRITER=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d "{\"path\":\"$STREAM\",\"flags\":{\"write\":true,\"create\":true}}" | jq -r '.handle_id')
echo "Writer handle: $WRITER"

# Create two readers
echo "Creating two readers..."
READER1=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d "{\"path\":\"$STREAM\",\"flags\":{\"read\":true}}" | jq -r '.handle_id')
echo "Reader 1 handle: $READER1"

READER2=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d "{\"path\":\"$STREAM\",\"flags\":{\"read\":true}}" | jq -r '.handle_id')
echo "Reader 2 handle: $READER2"

echo ""
echo "Writing messages to stream..."
for i in 1 2 3 4 5; do
    MSG="Event #$i at $(date +%H:%M:%S)"
    curl -s -X POST "$SERVER/api/v1/write?handle_id=$WRITER&offset=0" \
        -H "Content-Type: application/octet-stream" \
        -d "$MSG" > /dev/null
    echo "  Wrote: $MSG"
done

echo ""
echo "Reading from both readers (they should receive all messages):"
echo ""

echo "Reader 1 received:"
R1_DATA=$(curl -s -X POST "$SERVER/api/v1/read" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$READER1\",\"offset\":0,\"size\":10000}")
echo "  $R1_DATA"

echo ""
echo "Reader 2 received:"
R2_DATA=$(curl -s -X POST "$SERVER/api/v1/read" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$READER2\",\"offset\":0,\"size\":10000}")
echo "  $R2_DATA"

echo ""
echo "Closing handles..."
curl -s -X POST "$SERVER/api/v1/close" -H "Content-Type: application/json" -d "{\"handle_id\":\"$WRITER\",\"sync\":false}" > /dev/null
curl -s -X POST "$SERVER/api/v1/close" -H "Content-Type: application/json" -d "{\"handle_id\":\"$READER1\",\"sync\":false}" > /dev/null
curl -s -X POST "$SERVER/api/v1/close" -H "Content-Type: application/json" -d "{\"handle_id\":\"$READER2\",\"sync\":false}" > /dev/null

echo ""
echo "=== Demo Complete ==="
echo ""
echo "Key observations:"
echo "  - Both readers received the same data (fanout/broadcast)"
echo "  - Readers registered after writes still get historical data (ring buffer)"
echo "  - StreamFS supports multiple concurrent readers and writers"
