use jxl_grid::SimpleGrid;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct Region {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    #[inline]
    pub fn empty() -> Self {
        Self::default()
    }

    #[inline]
    pub fn with_size(width: u32, height: u32) -> Self {
        Self {
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    #[inline]
    pub fn right(self) -> i32 {
        self.left.saturating_add_unsigned(self.width)
    }

    #[inline]
    pub fn bottom(self) -> i32 {
        self.top.saturating_add_unsigned(self.height)
    }

    pub fn contains(self, target: Region) -> bool {
        if target.is_empty() {
            return true;
        }

        self.left <= target.left && self.top <= target.top && self.right() >= target.right() && self.bottom() >= target.bottom()
    }

    pub fn translate(self, x: i32, y: i32) -> Self {
        Self {
            left: self.left + x,
            top: self.top + y,
            ..self
        }
    }

    pub fn intersection(self, rhs: Region) -> Self {
        if self.width == 0 || rhs.width == 0 || self.height == 0 || rhs.height == 0 {
            return Self {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
            };
        }

        let mut ax = (self.left, self.right());
        let mut ay = (self.top, self.bottom());
        let mut bx = (rhs.left, rhs.right());
        let mut by = (rhs.top, rhs.bottom());
        if ax.0 > bx.0 {
            std::mem::swap(&mut ax, &mut bx);
        }
        if ay.0 > by.0 {
            std::mem::swap(&mut ay, &mut by);
        }

        if ax.1 <= bx.0 || ay.1 <= by.0 {
            Self {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
            }
        } else {
            Self {
                left: bx.0,
                top: by.0,
                width: std::cmp::min(ax.1, bx.1).abs_diff(bx.0),
                height: std::cmp::min(ay.1, by.1).abs_diff(by.0),
            }
        }
    }

    #[inline]
    pub fn pad(self, size: u32) -> Self {
        Self {
            left: self.left.saturating_sub_unsigned(size),
            top: self.top.saturating_sub_unsigned(size),
            width: self.width + size * 2,
            height: self.height + size * 2,
        }
    }

    #[inline]
    pub fn downsample(self, factor: u32) -> Self {
        if factor == 0 {
            return self;
        }

        let add = (1u32 << factor) - 1;
        let new_left = self.left >> factor;
        let new_top = self.top >> factor;
        let adj_width = self.width + self.left.abs_diff(new_left << factor);
        let adj_height = self.height + self.top.abs_diff(new_top << factor);
        Self {
            left: new_left,
            top: new_top,
            width: (adj_width + add) >> factor,
            height: (adj_height + add) >> factor,
        }
    }

    #[inline]
    pub fn downsample_separate(self, factor_x: u32, factor_y: u32) -> Self {
        if factor_x == 0 && factor_y == 0 {
            return self;
        }

        let add_x = (1u32 << factor_x) - 1;
        let new_left = self.left >> factor_x;
        let adj_width = self.width + self.left.abs_diff(new_left << factor_x);
        let add_y = (1u32 << factor_y) - 1;
        let new_top = self.top >> factor_y;
        let adj_height = self.height + self.top.abs_diff(new_top << factor_y);
        Self {
            left: new_left,
            top: new_top,
            width: (adj_width + add_x) >> factor_x,
            height: (adj_height + add_y) >> factor_y,
        }
    }

    #[inline]
    pub fn upsample(self, factor: u32) -> Self {
        self.upsample_separate(factor, factor)
    }

    #[inline]
    pub fn upsample_separate(self, factor_x: u32, factor_y: u32) -> Self {
        Self {
            left: self.left << factor_x,
            top: self.top << factor_y,
            width: self.width << factor_x,
            height: self.height << factor_y,
        }
    }

    pub(crate) fn container_aligned(self, grid_dim: u32) -> Self {
        debug_assert!(grid_dim.is_power_of_two());
        let add = grid_dim - 1;
        let mask = !add;
        let new_left = ((self.left as u32) & mask) as i32;
        let new_top = ((self.top as u32) & mask) as i32;
        let x_diff = self.left.abs_diff(new_left);
        let y_diff = self.top.abs_diff(new_top);
        Self {
            left: new_left,
            top: new_top,
            width: (self.width + x_diff + add) & mask,
            height: (self.height + y_diff + add) & mask,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageWithRegion {
    region: Region,
    buffer: Vec<SimpleGrid<f32>>,
}

impl ImageWithRegion {
    pub fn from_region(channels: usize, region: Region) -> Self {
        let width = region.width as usize;
        let height = region.height as usize;
        let buffer = std::iter::repeat_with(|| SimpleGrid::new(width, height))
            .take(channels)
            .collect();

        Self {
            region,
            buffer,
        }
    }

    pub fn from_buffer(buffer: Vec<SimpleGrid<f32>>, left: i32, top: i32) -> Self {
        let channels = buffer.len();
        if channels == 0 {
            Self {
                region: Region { top, left, width: 0, height: 0 },
                buffer,
            }
        } else {
            let width = buffer[0].width();
            let height = buffer[0].height();
            if buffer.iter().any(|g| g.width() != width || g.height() != height) {
                panic!("Buffer size is not uniform");
            }
            Self {
                region: Region { top, left, width: width as u32, height: height as u32 },
                buffer,
            }
        }
    }

    #[inline]
    pub fn channels(&self) -> usize {
        self.buffer.len()
    }

    #[inline]
    pub fn region(&self) -> Region {
        self.region
    }

    #[inline]
    pub fn buffer(&self) -> &[SimpleGrid<f32>] {
        &self.buffer
    }

    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [SimpleGrid<f32>] {
        &mut self.buffer
    }

    #[inline]
    pub fn take_buffer(&mut self) -> Vec<SimpleGrid<f32>> {
        std::mem::take(&mut self.buffer)
    }

    #[inline]
    pub fn add_channel(&mut self) -> &mut SimpleGrid<f32> {
        self.buffer.push(SimpleGrid::new(self.region.width as usize, self.region.height as usize));
        self.buffer.last_mut().unwrap()
    }

    #[inline]
    pub fn remove_channels(&mut self, range: impl std::ops::RangeBounds<usize>) {
        self.buffer.drain(range);
    }

    pub fn clone_intersection(&self, new_region: Region) -> Self {
        let intersection = self.region.intersection(new_region);
        let begin_x = intersection.left.abs_diff(self.region.left) as usize;
        let begin_y = intersection.top.abs_diff(self.region.top) as usize;
        let mut out = Self::from_region(self.channels(), intersection);

        for (input, output) in self.buffer.iter().zip(out.buffer.iter_mut()) {
            let input_stride = input.width();
            let input_buf = input.buf();
            let output_stride = output.width();
            let output_buf = output.buf_mut();
            for (input_row, output_row) in input_buf.chunks_exact(input_stride).skip(begin_y).zip(output_buf.chunks_exact_mut(output_stride)) {
                let input_row = &input_row[begin_x..][..output_stride];
                output_row.copy_from_slice(input_row);
            }
        }

        out
    }

    pub(crate) fn clone_region_channel(&self, out_region: Region, channel_idx: usize, output: &mut SimpleGrid<f32>) {
        if output.width() != out_region.width as usize || output.height() != out_region.height as usize {
            panic!("region mismatch, out_region={:?}, grid size={:?}", out_region, (output.width(), output.height()));
        }
        let input = self.buffer.get(channel_idx).expect("channel index out of range");

        let intersection = self.region.intersection(out_region);
        if intersection.is_empty() {
            return;
        }

        let input_begin_x = intersection.left.abs_diff(self.region.left) as usize;
        let input_begin_y = intersection.top.abs_diff(self.region.top) as usize;
        let output_begin_x = intersection.left.abs_diff(out_region.left).clamp(0, out_region.width) as usize;
        let output_begin_y = intersection.top.abs_diff(out_region.top).clamp(0, out_region.height) as usize;

        let input_stride = input.width();
        let input_buf = input.buf();
        let output_stride = output.width();
        let output_buf = output.buf_mut();
        for (input_row, output_row) in input_buf.chunks_exact(input_stride).skip(input_begin_y).zip(output_buf.chunks_exact_mut(output_stride).skip(output_begin_y)) {
            let input_row = &input_row[input_begin_x..][..intersection.width as usize];
            let output_row = &mut output_row[output_begin_x..][..intersection.width as usize];
            output_row.copy_from_slice(input_row);
        }
    }
}
