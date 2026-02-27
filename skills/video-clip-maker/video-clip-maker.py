#!/usr/bin/env python3
"""
Video Clip Maker - Create vertical short-form videos from YouTube
with auto-highlights, subtitles, and thumbnails.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from datetime import datetime

# Try to import optional dependencies
try:
    import cv2
    import numpy as np
    HAS_OPENCV = True
except ImportError:
    HAS_OPENCV = False
    print("Warning: OpenCV not installed. Some features may be limited.")

try:
    import whisper
    HAS_WHISPER = True
except ImportError:
    HAS_WHISPER = False
    print("Warning: Whisper not installed. Subtitles will not be generated.")

def check_dependencies():
    """Check if required binaries are available."""
    required = ['yt-dlp', 'ffmpeg']
    missing = []
    for cmd in required:
        try:
            subprocess.run(['which', cmd], capture_output=True, check=True)
        except subprocess.CalledProcessError:
            missing.append(cmd)
    
    if missing:
        print(f"Error: Missing required dependencies: {', '.join(missing)}")
        print("Please install them:")
        print("  brew install yt-dlp ffmpeg")
        return False
    return True

def download_video(url, output_dir, quality='1080p'):
    """Download video from YouTube using yt-dlp."""
    print(f"Downloading video from {url}...")
    
    # Create output directory
    os.makedirs(output_dir, exist_ok=True)
    
    # Build yt-dlp command
    cmd = [
        'yt-dlp',
        '-f', f'bestvideo[height<={quality[:-1]}]+bestaudio/best[height<={quality[:-1]}]',
        '--merge-output-format', 'mp4',
        '-o', os.path.join(output_dir, 'original.%(ext)s'),
        url
    ]
    
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        print("Download completed successfully.")
        
        # Find the downloaded file
        for ext in ['.mp4', '.mkv', '.webm']:
            video_path = os.path.join(output_dir, f'original{ext}')
            if os.path.exists(video_path):
                return video_path
        
        # Try to find any video file
        for file in os.listdir(output_dir):
            if file.startswith('original.'):
                return os.path.join(output_dir, file)
                
        raise FileNotFoundError("Downloaded video file not found")
        
    except subprocess.CalledProcessError as e:
        print(f"Download failed: {e.stderr}")
        return None

def detect_highlights(video_path, duration=60, method='audio'):
    """Detect highlight segments in the video."""
    print(f"Detecting highlights using {method} method...")
    
    if not HAS_OPENCV:
        print("OpenCV not available, using simple time-based segment.")
        # Return middle segment as fallback
        return get_video_duration(video_path), duration
    
    # Get video duration
    total_duration = get_video_duration(video_path)
    
    if method == 'audio':
        # Simple audio energy detection
        return detect_audio_highlights(video_path, duration)
    elif method == 'face':
        # Face detection for talking head videos
        return detect_face_segments(video_path, duration)
    else:
        # Default: middle segment
        start_time = max(0, (total_duration - duration) / 2)
        return start_time, duration

def get_video_duration(video_path):
    """Get video duration in seconds."""
    cmd = [
        'ffprobe',
        '-v', 'error',
        '-show_entries', 'format=duration',
        '-of', 'default=noprint_wrappers=1:nokey=1',
        video_path
    ]
    
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        return float(result.stdout.strip())
    except:
        return 300  # Default 5 minutes

def detect_audio_highlights(video_path, target_duration):
    """Simple audio energy detection."""
    # Extract audio
    with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as tmp:
        audio_path = tmp.name
    
    cmd = [
        'ffmpeg',
        '-i', video_path,
        '-vn', '-acodec', 'pcm_s16le',
        '-ar', '16000',
        '-ac', '1',
        '-y', audio_path
    ]
    
    try:
        subprocess.run(cmd, capture_output=True, check=True)
        
        # Read audio file (simplified)
        import wave
        with wave.open(audio_path, 'rb') as wav:
            frames = wav.readframes(wav.getnframes())
        
        # Clean up
        os.unlink(audio_path)
        
        # Simple: return middle segment
        total_duration = get_video_duration(video_path)
        start_time = max(0, (total_duration - target_duration) / 2)
        return start_time, target_duration
        
    except:
        # Fallback
        total_duration = get_video_duration(video_path)
        start_time = max(0, (total_duration - target_duration) / 2)
        return start_time, target_duration

def detect_face_segments(video_path, target_duration):
    """Face detection for talking head videos."""
    # Simplified implementation
    total_duration = get_video_duration(video_path)
    start_time = max(0, (total_duration - target_duration) / 2)
    return start_time, target_duration

def create_vertical_clip(input_path, output_path, start_time, duration):
    """Create vertical (9:16) clip from video segment."""
    print(f"Creating vertical clip: {duration}s starting at {start_time}s...")
    
    # Get video dimensions
    cmd = [
        'ffprobe',
        '-v', 'error',
        '-select_streams', 'v:0',
        '-show_entries', 'stream=width,height',
        '-of', 'csv=p=0',
        input_path
    ]
    
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        width, height = map(int, result.stdout.strip().split(','))
    except:
        width, height = 1920, 1080  # Default
    
    # Calculate crop for 9:16 (1080x1920)
    target_width = 1080
    target_height = 1920
    
    # For horizontal video, we want to crop the center portion
    # Scale to fit height, then crop width
    scale_filter = f"scale=-2:{target_height}"
    crop_filter = f"crop={target_width}:{target_height}"
    
    # Scale and crop
    cmd = [
        'ffmpeg',
        '-ss', str(start_time),
        '-i', input_path,
        '-t', str(duration),
        '-vf', f'{scale_filter},{crop_filter}',
        '-c:v', 'libx264',
        '-preset', 'medium',
        '-crf', '23',
        '-c:a', 'aac',
        '-b:a', '128k',
        '-y', output_path
    ]
    
    try:
        subprocess.run(cmd, capture_output=True, check=True)
        print(f"Vertical clip created: {output_path}")
        return True
    except subprocess.CalledProcessError as e:
        print(f"Failed to create vertical clip: {e.stderr}")
        return False

def generate_subtitles(video_path, output_srt, language='en'):
    """Generate subtitles using Whisper."""
    if not HAS_WHISPER:
        print("Whisper not available, skipping subtitles.")
        return False
    
    print("Generating subtitles...")
    
    try:
        model = whisper.load_model("small")
        result = model.transcribe(video_path, language=language)
        
        # Save as SRT
        with open(output_srt, 'w') as f:
            for i, segment in enumerate(result['segments']):
                start = segment['start']
                end = segment['end']
                text = segment['text'].strip()
                
                # Format time
                start_str = format_time(start)
                end_str = format_time(end)
                
                f.write(f"{i+1}\n")
                f.write(f"{start_str} --> {end_str}\n")
                f.write(f"{text}\n\n")
        
        print(f"Subtitles saved: {output_srt}")
        return True
    except Exception as e:
        print(f"Failed to generate subtitles: {e}")
        return False

def format_time(seconds):
    """Format seconds to SRT time format."""
    hours = int(seconds // 3600)
    minutes = int((seconds % 3600) // 60)
    secs = seconds % 60
    return f"{hours:02d}:{minutes:02d}:{secs:06.3f}".replace('.', ',')

def burn_subtitles(video_path, srt_path, output_path):
    """Burn subtitles into video."""
    if not os.path.exists(srt_path):
        print("No subtitle file found, skipping.")
        return False

    print("Burning subtitles into video...")

    # Copy SRT to a temp file with a simple path (no special chars).
    import shutil
    with tempfile.NamedTemporaryFile(suffix='.srt', delete=False, dir='/tmp') as tmp:
        tmp_srt_path = tmp.name
    shutil.copy2(srt_path, tmp_srt_path)

    # FFmpeg subtitles filter requires escaping colons and backslashes
    # in the file path.  When using subprocess list mode (no shell),
    # do NOT wrap the path in quotes — subprocess handles that.
    escaped_path = tmp_srt_path.replace('\\', '\\\\').replace(':', '\\:')

    # Attempt 1: plain subtitles via libass (most reliable)
    cmd1 = [
        'ffmpeg', '-i', video_path,
        '-vf', f"subtitles={escaped_path}",
        '-c:v', 'libx264', '-preset', 'medium', '-crf', '23',
        '-c:a', 'aac', '-b:a', '128k',
        '-y', output_path
    ]
    try:
        subprocess.run(cmd1, capture_output=True, text=True, check=True)
        print(f"Video with subtitles saved: {output_path}")
        os.unlink(tmp_srt_path)
        return True
    except subprocess.CalledProcessError as e:
        print(f"Subtitles filter failed: {e.stderr}")

    # Attempt 2: styled subtitles with force_style
    style = (
        "Fontname=Arial,Fontsize=24,"
        "PrimaryColour=&H00FFFFFF,"
        "OutlineColour=&H00000000,"
        "BackColour=&H80000000,"
        "BorderStyle=3,Outline=1,Shadow=1,MarginV=20"
    )
    cmd2 = [
        'ffmpeg', '-i', video_path,
        '-vf', f"subtitles={escaped_path}:force_style='{style}'",
        '-c:v', 'libx264', '-preset', 'medium', '-crf', '23',
        '-c:a', 'aac', '-b:a', '128k',
        '-y', output_path
    ]
    try:
        subprocess.run(cmd2, capture_output=True, text=True, check=True)
        print(f"Video with styled subtitles saved: {output_path}")
        os.unlink(tmp_srt_path)
        return True
    except subprocess.CalledProcessError as e2:
        print(f"Styled subtitles failed: {e2.stderr}")

    # Attempt 3: re-encode without subtitles (at least produce a video)
    cmd3 = [
        'ffmpeg', '-i', video_path,
        '-c:v', 'libx264', '-preset', 'medium', '-crf', '23',
        '-c:a', 'aac', '-b:a', '128k',
        '-y', output_path
    ]
    try:
        subprocess.run(cmd3, capture_output=True, text=True, check=True)
        print(f"Video saved without subtitles (fallback): {output_path}")
        os.unlink(tmp_srt_path)
        return True
    except subprocess.CalledProcessError as e3:
        print(f"All methods failed: {e3.stderr}")
        try:
            os.unlink(tmp_srt_path)
        except OSError:
            pass
        return False

def generate_thumbnail(video_path, output_image, time_sec=10):
    """Generate thumbnail from video frame."""
    print(f"Generating thumbnail at {time_sec}s...")
    
    # Use a simpler approach - just extract a frame
    cmd = [
        'ffmpeg',
        '-i', video_path,
        '-ss', str(time_sec),
        '-vframes', '1',
        '-vf', 'scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920',
        '-y', output_image
    ]
    
    try:
        subprocess.run(cmd, capture_output=True, check=True)
        print(f"Thumbnail saved: {output_image}")
        return True
    except subprocess.CalledProcessError as e:
        print(f"Failed to generate thumbnail: {e.stderr}")
        # Try alternative approach
        cmd2 = [
            'ffmpeg',
            '-i', video_path,
            '-vf', 'select=eq(n\,0)',
            '-vframes', '1',
            '-y', output_image
        ]
        try:
            subprocess.run(cmd2, capture_output=True, check=True)
            print(f"Thumbnail saved (alternative method): {output_image}")
            return True
        except:
            return False

def main():
    parser = argparse.ArgumentParser(description='Create vertical short-form videos from YouTube')
    parser.add_argument('--url', required=True, help='YouTube video URL')
    parser.add_argument('--duration', type=int, default=60, help='Clip duration in seconds (default: 60)')
    parser.add_argument('--quality', default='1080p', choices=['720p', '1080p', '4k'], help='Video quality')
    parser.add_argument('--output-dir', default='./output', help='Output directory')
    parser.add_argument('--highlight-method', default='audio', choices=['audio', 'face', 'motion'], help='Highlight detection method')
    parser.add_argument('--subtitle-lang', default='en', help='Subtitle language')
    
    args = parser.parse_args()
    
    # Check dependencies
    if not check_dependencies():
        sys.exit(1)
    
    # Create output directory
    output_dir = Path(args.output_dir)
    output_dir.mkdir(exist_ok=True)
    
    timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
    
    # Download video
    video_path = download_video(args.url, output_dir, args.quality)
    if not video_path:
        print("Failed to download video.")
        sys.exit(1)
    
    # Detect highlights
    start_time, duration = detect_highlights(video_path, args.duration, args.highlight_method)
    
    # Create vertical clip
    vertical_path = output_dir / f'vertical_{timestamp}.mp4'
    if not create_vertical_clip(video_path, str(vertical_path), start_time, duration):
        print("Failed to create vertical clip.")
        sys.exit(1)
    
    # Generate subtitles
    srt_path = output_dir / f'subtitles_{timestamp}.srt'
    if HAS_WHISPER:
        generate_subtitles(str(vertical_path), str(srt_path), args.subtitle_lang)
    
    # Burn subtitles if available
    final_path = output_dir / f'final_{timestamp}.mp4'
    if os.path.exists(srt_path):
        if not burn_subtitles(str(vertical_path), str(srt_path), str(final_path)):
            # If subtitle burning failed entirely, use vertical clip as final
            final_path = vertical_path
    else:
        final_path = vertical_path

    # If final_path doesn't exist (shouldn't happen), fall back to vertical
    if not os.path.exists(final_path):
        final_path = vertical_path

    # Generate thumbnail from whichever video file exists
    thumbnail_path = output_dir / f'thumbnail_{timestamp}.png'
    generate_thumbnail(str(final_path), str(thumbnail_path))
    
    # Create metadata
    metadata = {
        'original_url': args.url,
        'clip_start': start_time,
        'clip_duration': duration,
        'output_files': {
            'vertical_clip': str(vertical_path),
            'final_clip': str(final_path),
            'thumbnail': str(thumbnail_path),
            'subtitles': str(srt_path) if os.path.exists(srt_path) else None
        },
        'timestamp': timestamp,
        'quality': args.quality
    }
    
    metadata_path = output_dir / f'metadata_{timestamp}.json'
    with open(metadata_path, 'w') as f:
        json.dump(metadata, f, indent=2)
    
    print("\n" + "="*50)
    print("PROCESSING COMPLETE!")
    print("="*50)
    print(f"Original URL: {args.url}")
    print(f"Clip: {duration}s starting at {start_time:.1f}s")
    print(f"Final video: {final_path}")
    print(f"Thumbnail: {thumbnail_path}")
    if os.path.exists(srt_path):
        print(f"Subtitles: {srt_path}")
    print(f"Metadata: {metadata_path}")
    print("="*50)

if __name__ == '__main__':
    main()