# Linggen Assets

## App Icon

To create the macOS app icon:

1. Create a 1024x1024 PNG icon (e.g., `icon.png`)

2. Convert to `.icns` format:

   ```bash
   # Create iconset directory
   mkdir icon.iconset

   # Generate all required sizes
   sips -z 16 16     icon.png --out icon.iconset/icon_16x16.png
   sips -z 32 32     icon.png --out icon.iconset/icon_16x16@2x.png
   sips -z 32 32     icon.png --out icon.iconset/icon_32x32.png
   sips -z 64 64     icon.png --out icon.iconset/icon_32x32@2x.png
   sips -z 128 128   icon.png --out icon.iconset/icon_128x128.png
   sips -z 256 256   icon.png --out icon.iconset/icon_128x128@2x.png
   sips -z 256 256   icon.png --out icon.iconset/icon_256x256.png
   sips -z 512 512   icon.png --out icon.iconset/icon_256x256@2x.png
   sips -z 512 512   icon.png --out icon.iconset/icon_512x512.png
   sips -z 1024 1024 icon.png --out icon.iconset/icon_512x512@2x.png

   # Convert to icns
   iconutil -c icns icon.iconset -o icon.icns

   # Clean up
   rm -rf icon.iconset
   ```

3. Place `icon.icns` in this directory

The build script will automatically include it in the app bundle.
