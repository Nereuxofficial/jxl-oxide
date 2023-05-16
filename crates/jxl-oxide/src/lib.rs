use std::{
    fs::File,
    io::Read,
    path::Path,
};

mod fb;

pub use jxl_bitstream as bitstream;
pub use jxl_color::header as color;
pub use jxl_image as image;
pub use jxl_frame::header as frame;

pub use jxl_bitstream::{Bitstream, Bundle};
use jxl_bitstream::{ContainerDetectingReader, Name};
pub use jxl_frame::{Frame, FrameHeader};
pub use jxl_grid::SimpleGrid;
pub use jxl_image::{ExtraChannelType, ImageHeader};
use jxl_render::RenderContext;

pub use fb::FrameBuffer;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

#[derive(Debug)]
pub struct JxlImage<R> {
    bitstream: Bitstream<ContainerDetectingReader<R>>,
    image_header: ImageHeader,
    embedded_icc: Option<Vec<u8>>,
}

impl<R: Read> JxlImage<R> {
    /// Creates a `JxlImage` from the reader.
    pub fn from_reader(reader: R) -> Result<Self> {
        let mut bitstream = Bitstream::new_detect(reader);
        let image_header = ImageHeader::parse(&mut bitstream, ())?;

        let embedded_icc = image_header.metadata.colour_encoding.want_icc.then(|| {
            tracing::debug!("Image has an embedded ICC profile");
            let icc = jxl_color::icc::read_icc(&mut bitstream)?;
            jxl_color::icc::decode_icc(&icc)
        }).transpose()?;

        if image_header.metadata.preview.is_some() {
            tracing::debug!("Skipping preview frame");
            bitstream.zero_pad_to_byte()?;

            let frame = Frame::parse(&mut bitstream, &image_header)?;
            let toc = frame.toc();
            let bookmark = toc.bookmark() + (toc.total_byte_size() * 8);
            bitstream.skip_to_bookmark(bookmark)?;
        }

        Ok(Self {
            bitstream,
            image_header,
            embedded_icc,
        })
    }

    /// Returns the image header.
    #[inline]
    pub fn image_header(&self) -> &ImageHeader {
        &self.image_header
    }

    /// Returns the embedded ICC profile.
    #[inline]
    pub fn embedded_icc(&self) -> Option<&[u8]> {
        self.embedded_icc.as_deref()
    }

    /// Starts rendering the image.
    #[inline]
    pub fn renderer(&mut self) -> JxlRenderer<'_, R> {
        let ctx = RenderContext::new(&self.image_header);
        JxlRenderer {
            bitstream: &mut self.bitstream,
            image_header: &self.image_header,
            embedded_icc: self.embedded_icc.as_deref(),
            ctx,
            crop_region: None,
            end_of_image: false,
        }
    }
}

impl JxlImage<File> {
    /// Creates a `JxlImage` from the filesystem.
    #[inline]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        Self::from_reader(file)
    }
}

#[derive(Debug)]
pub struct JxlRenderer<'img, R> {
    bitstream: &'img mut Bitstream<ContainerDetectingReader<R>>,
    image_header: &'img ImageHeader,
    embedded_icc: Option<&'img [u8]>,
    ctx: RenderContext<'img>,
    crop_region: Option<CropInfo>,
    end_of_image: bool,
}

impl<'img, R: Read> JxlRenderer<'img, R> {
    /// Returns the image header.
    #[inline]
    pub fn image_header(&self) -> &'img ImageHeader {
        self.image_header
    }

    /// Sets the cropping region of the image.
    #[inline]
    pub fn set_crop_region(&mut self, crop_region: Option<CropInfo>) -> &mut Self {
        self.crop_region = crop_region;
        self
    }

    /// Returns the cropping region of the image.
    #[inline]
    pub fn crop_region(&self) -> Option<CropInfo> {
        self.crop_region
    }

    #[inline]
    fn crop_region_flattened(&self) -> Option<(u32, u32, u32, u32)> {
        self.crop_region.map(|info| (info.left, info.top, info.width, info.height))
    }

    /// Returns the embedded ICC profile.
    #[inline]
    pub fn embedded_icc(&self) -> Option<&'img [u8]> {
        self.embedded_icc
    }

    /// Returns the ICC profile that describes rendered images.
    ///
    /// - If the image is XYB encoded, and the ICC profile is embedded, then the profile describes
    ///   linear sRGB or linear grayscale colorspace.
    /// - Else, if the ICC profile is embedded, then the embedded profile is returned.
    /// - Else, the profile describes the color encoding signalled in the image header.
    pub fn rendered_icc(&self) -> Vec<u8> {
        if !self.image_header.metadata.xyb_encoded {
            if let Some(icc) = self.embedded_icc {
                return icc.to_vec();
            }
        }

        jxl_color::icc::colour_encoding_to_icc(&self.image_header.metadata.colour_encoding)
    }

    /// Returns the pixel format of the rendered image.
    pub fn pixel_format(&self) -> PixelFormat {
        let is_grayscale = self.image_header.metadata.grayscale();
        let mut has_black = false;
        let mut has_alpha = false;
        for ec_info in &self.image_header.metadata.ec_info {
            if ec_info.is_alpha() {
                has_alpha = true;
            }
            if ec_info.is_black() {
                has_black = true;
            }
        }

        match (is_grayscale, has_black, has_alpha) {
            (false, false, false) => PixelFormat::Rgb,
            (false, false, true) => PixelFormat::Rgba,
            (false, true, false) => PixelFormat::Cmyk,
            (false, true, true) => PixelFormat::Cmyka,
            (true, _, false) => PixelFormat::Gray,
            (true, _, true) => PixelFormat::Graya,
        }
    }

    /// Renders the next keyframe.
    pub fn render_next_frame(&mut self) -> Result<RenderResult> {
        if self.end_of_image {
            return Ok(RenderResult::NoMoreFrames);
        }

        let region = self.crop_region_flattened();
        let result = self.ctx.load_until_keyframe(self.bitstream, false, region)?;
        match result {
            jxl_frame::ProgressiveResult::NeedMoreData => Ok(RenderResult::NeedMoreData),
            jxl_frame::ProgressiveResult::FrameComplete => {
                let keyframe_index = self.ctx.loaded_keyframes() - 1;
                let mut grids = self.ctx.render_keyframe_cropped(keyframe_index, region)?;

                let color_channels = if self.image_header.metadata.grayscale() { 1 } else { 3 };
                let color_channels: Vec<_> = grids.drain(..color_channels).collect();
                let extra_channels: Vec<_> = grids
                    .into_iter()
                    .zip(&self.image_header.metadata.ec_info)
                    .map(|(grid, ec_info)| ExtraChannel {
                        ty: ec_info.ty,
                        name: ec_info.name.clone(),
                        grid,
                    })
                    .collect();

                let frame = self.ctx.keyframe(keyframe_index).unwrap();
                let frame_header = frame.header();
                let result = Render {
                    keyframe_index,
                    name: frame_header.name.clone(),
                    duration: frame_header.duration,
                    orientation: self.image_header.metadata.orientation,
                    color_channels,
                    extra_channels,
                };

                self.end_of_image = frame_header.is_last;
                Ok(RenderResult::Done(result))
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum PixelFormat {
    Gray,
    Graya,
    Rgb,
    Rgba,
    Cmyk,
    Cmyka,
}

impl PixelFormat {
    #[inline]
    pub fn channels(self) -> usize {
        match self {
            PixelFormat::Gray => 1,
            PixelFormat::Graya => 2,
            PixelFormat::Rgb => 3,
            PixelFormat::Rgba => 4,
            PixelFormat::Cmyk => 4,
            PixelFormat::Cmyka => 5,
        }
    }

    #[inline]
    pub fn has_alpha(self) -> bool {
        matches!(self, PixelFormat::Graya | PixelFormat::Rgba | PixelFormat::Cmyka)
    }

    #[inline]
    pub fn has_black(self) -> bool {
        matches!(self, PixelFormat::Cmyk | PixelFormat::Cmyka)
    }
}

#[derive(Debug)]
pub enum RenderResult {
    Done(Render),
    NeedMoreData,
    NoMoreFrames,
}

#[derive(Debug)]
pub struct Render {
    keyframe_index: usize,
    name: Name,
    duration: u32,
    orientation: u32,
    color_channels: Vec<SimpleGrid<f32>>,
    extra_channels: Vec<ExtraChannel>,
}

impl Render {
    #[inline]
    pub fn keyframe_index(&self) -> usize {
        self.keyframe_index
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn duration(&self) -> u32 {
        self.duration
    }

    #[inline]
    pub fn orientation(&self) -> u32 {
        self.orientation
    }

    #[inline]
    pub fn image(&self) -> FrameBuffer {
        let mut fb: Vec<_> = self.color_channels.clone();

        // Find black
        for ec in &self.extra_channels {
            if ec.is_black() {
                fb.push(ec.grid.clone());
                break;
            }
        }
        // Find alpha
        for ec in &self.extra_channels {
            if ec.is_alpha() {
                fb.push(ec.grid.clone());
                break;
            }
        }

        FrameBuffer::from_grids(&fb, self.orientation)
    }

    #[inline]
    pub fn color_channels(&self) -> &[SimpleGrid<f32>] {
        &self.color_channels
    }

    #[inline]
    pub fn color_channels_mut(&mut self) -> &mut [SimpleGrid<f32>] {
        &mut self.color_channels
    }

    #[inline]
    pub fn extra_channels(&self) -> &[ExtraChannel] {
        &self.extra_channels
    }

    #[inline]
    pub fn extra_channels_mut(&mut self) -> &mut [ExtraChannel] {
        &mut self.extra_channels
    }
}

#[derive(Debug)]
pub struct ExtraChannel {
    ty: ExtraChannelType,
    name: Name,
    grid: SimpleGrid<f32>,
}

impl ExtraChannel {
    #[inline]
    pub fn ty(&self) -> ExtraChannelType {
        self.ty
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn grid(&self) -> &SimpleGrid<f32> {
        &self.grid
    }

    #[inline]
    pub fn grid_mut(&mut self) -> &mut SimpleGrid<f32> {
        &mut self.grid
    }

    #[inline]
    pub fn is_black(&self) -> bool {
        matches!(self.ty, ExtraChannelType::Black)
    }

    #[inline]
    pub fn is_alpha(&self) -> bool {
        matches!(self.ty, ExtraChannelType::Alpha { .. })
    }
}

#[derive(Debug, Default, Copy, Clone)]
pub struct CropInfo {
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub top: u32,
}
