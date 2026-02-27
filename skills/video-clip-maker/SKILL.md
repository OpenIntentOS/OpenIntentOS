---
name: video-clip-maker
description: Create vertical short-form videos from YouTube with auto-highlights, subtitles, and thumbnails
version: "1.0.0"
requires:
  env: []
  bins: ["yt-dlp", "ffmpeg", "python3"]
tags:
  - video
  - youtube
  - automation
  - social-media
author: OpenIntentOS Contributors
---

# Video Clip Maker

Automatically creates vertical short-form videos (9:16) from YouTube videos with AI-powered highlight detection, auto-generated subtitles, and thumbnail generation.

## Features

### 🎬 **YouTube Download**
- Download videos from YouTube using yt-dlp
- Support for playlists and channels
- Quality selection (up to 4K)

### ✂️ **Auto Highlight Detection**
- Scene change detection
- Audio energy analysis for exciting moments
- Face detection for talking head videos
- Motion-based segment scoring

### 📱 **Vertical Crop (9:16)**
- Smart center crop with subject tracking
- Optional blur background fill
- Maintains quality with upscaling

### 📝 **Auto Subtitles**
- Whisper AI speech-to-text
- Multiple language support
- SRT/VTT format output
- Burned-in subtitle styling

### 🖼️ **Thumbnail Generation**
- Key frame extraction
- Text overlay with title
- Optimized for social platforms

## Usage

### Basic Usage
```bash
# Download and process a YouTube video
python video-clip-maker.py --url "https://youtube.com/watch?v=VIDEO_ID"

# With custom settings
python video-clip-maker.py \
  --url "https://youtube.com/watch?v=VIDEO_ID" \
  --duration 60 \
  --output-format mp4 \
  --quality 1080p
```

### Advanced Options
```bash
# Specify highlight preferences
python video-clip-maker.py \
  --url "URL" \
  --highlight-method "audio" \
  --min-score 0.7

# Custom subtitle settings
python video-clip-maker.py \
  --url "URL" \
  --subtitle-lang "en" \
  --subtitle-style "bold" \
  --subtitle-size 24

# Batch processing
python video-clip-maker.py \
  --playlist "PLAYLIST_URL" \
  --max-videos 10
```

## Output Format

The skill produces:
- **Vertical video** (1080x1920, 9:16 aspect ratio)
- **Burned-in subtitles** (customizable style)
- **Thumbnail image** (1080x1920 PNG)
- **Metadata JSON** with processing info

## Configuration

### Environment Variables
- `WHISPER_MODEL`: Whisper model size (default: "small")
- `FFMPEG_PRESET`: Encoding preset (default: "medium")
- `OUTPUT_DIR`: Output directory (default: "./output")

### Quality Presets
| Preset | Resolution | File Size | Use Case |
|--------|------------|-----------|----------|
| `720p` | 720x1280 | Small | Quick sharing |
| `1080p` | 1080x1920 | Medium | Standard quality |
| `4k` | 2160x3840 | Large | Professional |

## Examples

### Example 1: Quick Clip
```bash
python video-clip-maker.py --url "https://youtube.com/watch?v=dQw4w9WgXcQ" --duration 30
```

### Example 2: Podcast Clip
```bash
python video-clip-maker.py \
  --url "https://youtube.com/watch?v=PODCAST_ID" \
  --highlight-method "face" \
  --subtitle-style "netflix" \
  --duration 60
```

### Example 3: Gaming Clip
```bash
python video-clip-maker.py \
  --url "https://youtube.com/watch?v=GAME_ID" \
  --highlight-method "motion" \
  --min-score 0.8 \
  --quality 1080p
```

## Technical Details

### Dependencies
- `yt-dlp`: Video downloading
- `ffmpeg`: Video processing
- `whisper`: Speech-to-text
- `opencv-python`: Video analysis
- `numpy`: Numerical operations

### Processing Pipeline
1. **Download**: Fetch video from YouTube
2. **Analyze**: Detect scenes, faces, motion, audio peaks
3. **Select**: Choose best highlight segment
4. **Crop**: Convert to 9:16 vertical format
5. **Subtitle**: Generate and burn in subtitles
6. **Export**: Output final video and thumbnail

## Limitations

- YouTube videos must be publicly accessible or authenticated
- Processing time: ~1-2x video length
- Whisper models require 2-4GB RAM
- 4K processing requires significant disk space

## Troubleshooting

### Common Issues
- **Download fails**: Check video URL and region restrictions
- **No audio detected**: Verify video has audio track
- **Subtitle errors**: Ensure whisper model is downloaded
- **Crop looks wrong**: Adjust crop method for content type

## Best Practices

1. **Source Quality**: Start with highest quality source
2. **Duration**: Keep clips under 60 seconds for best engagement
3. **Subtitles**: Always include for silent autoplay
4. **Thumbnails**: Use clear, high-contrast frames

## Integration

This skill integrates with:
- **Telegram**: Send clips directly to channels
- **YouTube Shorts**: Auto-upload processed clips
- **TikTok**: Format-optimized exports
- **Instagram Reels**: Aspect ratio presets
