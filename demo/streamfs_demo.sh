#!/bin/bash
# StreamFS Demo with FFmpeg

SERVER="http://localhost:9999"

echo "=== StreamFS Demo ==="
echo ""

# Test 1: Basic write/read
echo "Test 1: Basic text streaming"
echo "----------------------------"

# Open for write
WRITE_RESP=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/streamfs/test","flags":{"read":false,"write":true,"create":true}}')
WRITE_HANDLE=$(echo "$WRITE_RESP" | jq -r '.handle_id')
echo "Opened write handle: $WRITE_HANDLE"

# Write some data
curl -s -X POST "$SERVER/api/v1/write?handle_id=$WRITE_HANDLE&offset=0" \
    -H "Content-Type: application/octet-stream" \
    -d "Hello from StreamFS!" > /dev/null
echo "Wrote: Hello from StreamFS!"

# Open for read
READ_RESP=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/streamfs/test","flags":{"read":true,"write":false}}')
READ_HANDLE=$(echo "$READ_RESP" | jq -r '.handle_id')
echo "Opened read handle: $READ_HANDLE"

# Read data back
CONTENT=$(curl -s -X POST "$SERVER/api/v1/read" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$READ_HANDLE\",\"offset\":0,\"size\":1024}")
echo "Read back: $CONTENT"

# Close handles
curl -s -X POST "$SERVER/api/v1/close" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$WRITE_HANDLE\",\"sync\":false}" > /dev/null
curl -s -X POST "$SERVER/api/v1/close" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$READ_HANDLE\",\"sync\":false}" > /dev/null
echo ""

# Test 2: List streams
echo "Test 2: List streams"
echo "--------------------"
curl -s "$SERVER/api/v1/readdir?path=/streamfs" | jq -r '.[].path'
echo ""

# Test 3: Stream audio with ffmpeg
echo "Test 3: Audio streaming demo"
echo "----------------------------"

# Generate audio file locally first
echo "Generating 1 second test tone with ffmpeg..."
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=1" -f wav -ac 1 -ar 22050 /tmp/test_tone.wav 2>/dev/null
echo "Generated test tone: $(wc -c < /tmp/test_tone.wav) bytes"

# Open stream for write
AUDIO_WRITE=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/streamfs/audio.wav","flags":{"read":false,"write":true,"create":true}}')
AUDIO_HANDLE=$(echo "$AUDIO_WRITE" | jq -r '.handle_id')
echo "Opened audio stream: $AUDIO_HANDLE"

# Write audio data
WRITTEN=$(curl -s -X POST "$SERVER/api/v1/write?handle_id=$AUDIO_HANDLE&offset=0" \
    -H "Content-Type: application/octet-stream" \
    --data-binary @/tmp/test_tone.wav)
echo "Written response: $WRITTEN"

# Close write handle
curl -s -X POST "$SERVER/api/v1/close" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$AUDIO_HANDLE\",\"sync\":false}" > /dev/null

# Open for read
AUDIO_READ=$(curl -s -X POST "$SERVER/api/v1/open" \
    -H "Content-Type: application/json" \
    -d '{"path":"/streamfs/audio.wav","flags":{"read":true,"write":false}}')
AUDIO_READ_HANDLE=$(echo "$AUDIO_READ" | jq -r '.handle_id')
echo "Opened for reading: $AUDIO_READ_HANDLE"

# Read audio back
curl -s -X POST "$SERVER/api/v1/read" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$AUDIO_READ_HANDLE\",\"offset\":0,\"size\":100000}" \
    -o /tmp/streamfs_audio.wav

echo "Read back audio: $(wc -c < /tmp/streamfs_audio.wav) bytes"

# Close read handle
curl -s -X POST "$SERVER/api/v1/close" \
    -H "Content-Type: application/json" \
    -d "{\"handle_id\":\"$AUDIO_READ_HANDLE\",\"sync\":false}" > /dev/null

# Play the audio
if [ -s /tmp/streamfs_audio.wav ]; then
    echo "Playing audio with ffplay..."
    timeout 3 ffplay -nodisp -autoexit /tmp/streamfs_audio.wav 2>/dev/null
    echo "Audio playback complete!"
else
    echo "No audio data received"
fi

echo ""
echo "Test 4: List all streams"
echo "------------------------"
curl -s "$SERVER/api/v1/readdir?path=/streamfs" | jq '.'

echo ""
echo "=== Demo Complete ==="

# Cleanup
rm -f /tmp/test_tone.wav /tmp/streamfs_audio.wav
