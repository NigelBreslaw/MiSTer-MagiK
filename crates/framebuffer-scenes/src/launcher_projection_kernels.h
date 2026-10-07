// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// Bindings choose the quantiser, optional flat inks and compositor
// at compile time while geometry, bounds, phase alignment and tails stay shared.
static inline uint16x4_t MAGIK_PACK_ROW4(uint32x4_t p,size_t x,size_t y MAGIK_PALETTE_PARAMS) {
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[y&3][(x+j)&3]);
  return MAGIK_PACK4(p,vld1_u16(offsets));
}

void MAGIK_COLUMN_KERNEL(uint16_t *out,size_t pitch,const uint32_t *src,
    size_t height,size_t rows,int32_t q,int32_t step,size_t x,size_t y0 MAGIK_PALETTE_PARAMS) {
  size_t y=0;
  // Four vertical outputs fill every quantiser lane. The Bayer phase repeats
  // after four rows, so these offsets stay outside the interior loop.
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[(y0+j)&3][x&3]);
  const uint16x4_t phase=vld1_u16(offsets);
  for(;y+3<rows;y+=4) {
    int32_t q1=q+step,q2=q1+step,q3=q2+step;
    int32_t r=q>>16,r3=q3>>16;
    if(r>=0 && (size_t)(r3+1)<height) {
      uint32x4_t p=vcombine_u32(interpolate2(src,q,q1),interpolate2(src,q2,q3));
      uint32x4_t alpha=vshrq_n_u32(p,24);
      uint32x2_t minimum=vmin_u32(vget_low_u32(alpha),vget_high_u32(alpha));
      if(vget_lane_u32(minimum,0)==255 && vget_lane_u32(minimum,1)==255) {
        uint16x4_t packed=MAGIK_PACK4(p,phase);
        out[y*pitch]=vget_lane_u16(packed,0);
        out[(y+1)*pitch]=vget_lane_u16(packed,1);
        out[(y+2)*pitch]=vget_lane_u16(packed,2);
        out[(y+3)*pitch]=vget_lane_u16(packed,3);
      } else {
#if MAGIK_VECTOR_ALPHA
        uint16x4_t dst=vdup_n_u16(0);
        dst=vld1_lane_u16(out+y*pitch,dst,0);
        dst=vld1_lane_u16(out+(y+1)*pitch,dst,1);
        dst=vld1_lane_u16(out+(y+2)*pitch,dst,2);
        dst=vld1_lane_u16(out+(y+3)*pitch,dst,3);
        uint16x4_t packed=fast_over4(p,dst,phase);
        vst1_lane_u16(out+y*pitch,packed,0);
        vst1_lane_u16(out+(y+1)*pitch,packed,1);
        vst1_lane_u16(out+(y+2)*pitch,packed,2);
        vst1_lane_u16(out+(y+3)*pitch,packed,3);
#else
        uint32_t pixels[4];vst1q_u32(pixels,p);
        for(size_t j=0;j<4;++j) out[(y+j)*pitch]=MAGIK_PIXEL(pixels[j],out[(y+j)*pitch],x,y0+y+j);
#endif
      }
    } else {
      int32_t qj=q;
      for(size_t j=0;j<4;++j,qj+=step) {
        int32_t rj=qj>>16;
        uint32_t a=rj>=0 && (size_t)rj<height?src[rj]:0;
        uint32_t b=rj+1>=0 && (size_t)(rj+1)<height?src[rj+1]:0;
        out[(y+j)*pitch]=MAGIK_PIXEL(scalar(a,b,((uint32_t)qj&65535)>>8),out[(y+j)*pitch],x,y0+y+j);
      }
    }
    q=q3+step;
  }
  for (;y+1<rows;y+=2) {
    int32_t r=q>>16, q1=q+step, r1=q1>>16;
    if (r>=0 && r1>=0 && (size_t)(r+1)<height && (size_t)(r1+1)<height) {
      uint32x2_t p=interpolate2(src,q,q1);
      uint32x2_t alpha=vshr_n_u32(p,24);
      if (vget_lane_u32(alpha,0)==255 && vget_lane_u32(alpha,1)==255) {
        uint32x2_t packed=MAGIK_PACK2(p,x,y0+y);
        out[y*pitch]=(uint16_t)vget_lane_u32(packed,0);
        out[(y+1)*pitch]=(uint16_t)vget_lane_u32(packed,1);
      } else {
        out[y*pitch]=MAGIK_PIXEL(vget_lane_u32(p,0),out[y*pitch],x,y0+y);
        out[(y+1)*pitch]=MAGIK_PIXEL(vget_lane_u32(p,1),out[(y+1)*pitch],x,y0+y+1);
      }
    } else {
      for (size_t j=0;j<2;++j) {
        int32_t qj=q+(int32_t)j*step,rj=qj>>16;
        uint32_t a=rj>=0 && (size_t)rj<height ? src[rj]:0;
        uint32_t b=rj+1>=0 && (size_t)(rj+1)<height ? src[rj+1]:0;
        out[(y+j)*pitch]=MAGIK_PIXEL(scalar(a,b,((uint32_t)qj&65535)>>8),out[(y+j)*pitch],x,y0+y+j);
      }
    }
    q=q1+step;
  }
  if (y<rows) {
    int32_t r=q>>16;
    uint32_t a=r>=0 && (size_t)r<height ? src[r]:0;
    uint32_t b=r+1>=0 && (size_t)(r+1)<height ? src[r+1]:0;
    out[y*pitch]=MAGIK_PIXEL(scalar(a,b,((uint32_t)q&65535)>>8),out[y*pitch],x,y0+y);
  }
}

// Canonical card columns have an opaque source interior. The two bilinear
// inputs must both lie in it; rounded caps and arbitrary-alpha callers retain
// the generic kernel above. Work out output intervals once, not per vector.
void MAGIK_OPAQUE_KERNEL(uint16_t *out,size_t pitch,
    const uint32_t *src,size_t height,size_t rows,int32_t q,int32_t step,
    size_t x,size_t y0,size_t opaque_top,size_t opaque_bottom MAGIK_PALETTE_PARAMS) {
  if (!rows) return;
  if (step<=0 || opaque_top>=opaque_bottom || opaque_bottom>height ||
      (src[opaque_top]>>24)!=255 || (src[opaque_bottom-1]>>24)!=255) {
    MAGIK_COLUMN_KERNEL(out,pitch,src,height,rows,q,step,x,y0 MAGIK_PALETTE_ARGS);
    return;
  }
  size_t first=projected_row_boundary((int64_t)opaque_top<<16,q,step,rows);
  size_t end=projected_row_boundary((int64_t)(opaque_bottom-1)<<16,q,step,rows);
  if(first>=end) {
    MAGIK_COLUMN_KERNEL(out,pitch,src,height,rows,q,step,x,y0 MAGIK_PALETTE_ARGS);
    return;
  }
  MAGIK_COLUMN_KERNEL(out,pitch,src,height,first,q,step,x,y0 MAGIK_PALETTE_ARGS);
  int32_t sample=(int32_t)((int64_t)q+(int64_t)first*step);
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j)offsets[j]=(uint16_t)(256-image_threshold[(y0+first+j)&3][x&3]);
  const uint16x4_t phase=vld1_u16(offsets);
  size_t y=first;
  for(;y+3<end;y+=4) {
    int32_t q1=sample+step,q2=q1+step,q3=q2+step;
    uint32x4_t p=vcombine_u32(interpolate2(src,sample,q1),interpolate2(src,q2,q3));
    uint16x4_t packed=MAGIK_PACK4(p,phase);
    out[y*pitch]=vget_lane_u16(packed,0);
    out[(y+1)*pitch]=vget_lane_u16(packed,1);
    out[(y+2)*pitch]=vget_lane_u16(packed,2);
    out[(y+3)*pitch]=vget_lane_u16(packed,3);
    sample=q3+step;
  }
  if(y+1<end) {
    uint32x2_t p=interpolate2(src,sample,sample+step);
    uint32x2_t packed=MAGIK_PACK2(p,x,y0+y);
    out[y*pitch]=(uint16_t)vget_lane_u32(packed,0);
    out[(y+1)*pitch]=(uint16_t)vget_lane_u32(packed,1);
    y+=2;sample+=2*step;
  }
  if(y<end) {
    uint32x2_t packed=MAGIK_PACK2(interpolate2(src,sample,sample),x,y0+y);
    out[y*pitch]=(uint16_t)vget_lane_u32(packed,0);
  }
  if(end<rows) MAGIK_COLUMN_KERNEL(out+end*pitch,pitch,src,height,rows-end,
    (int32_t)((int64_t)q+(int64_t)end*step),step,x,y0+end MAGIK_PALETTE_ARGS);
}

// Flat cards use contiguous four-pixel stores, as in the production flat path.
void MAGIK_FLAT_KERNEL(uint16_t *out, size_t pitch,
    const uint32_t *src, size_t stride, size_t height, size_t width,
    size_t rows, int32_t q, int32_t step, size_t x0, size_t y0 MAGIK_PALETTE_PARAMS) {
  for (size_t y = 0; y < rows; ++y, q += step) {
    int32_t r = q >> 16;
    uint32_t w = ((uint32_t)q & 65535) >> 8;
    size_t x = 0;
    if (r >= 0 && (size_t)(r + 1) < height) {
      for (; x + 3 < width; x += 4) {
        const uint32_t *p0 = src + x * stride + r;
        const uint32_t *p1 = p0 + stride, *p2 = p1 + stride, *p3 = p2 + stride;
        uint32x2x2_t a = vtrn_u32(vld1_u32(p0), vld1_u32(p1));
        uint32x2x2_t b = vtrn_u32(vld1_u32(p2), vld1_u32(p3));
        uint32x4_t p = blend(vcombine_u32(a.val[0], b.val[0]),
                            vcombine_u32(a.val[1], b.val[1]), w);
        uint32x4_t alpha = vshrq_n_u32(p, 24);
        uint32x2_t m = vmin_u32(vget_low_u32(alpha), vget_high_u32(alpha));
        if (vget_lane_u32(m, 0) == 255 && vget_lane_u32(m, 1) == 255) {
          vst1_u16(out + y * pitch + x, MAGIK_PACK_ROW4(p, x0 + x, y0 + y MAGIK_PALETTE_ARGS));
        } else {
#if MAGIK_VECTOR_ALPHA
          uint16_t offsets[4];
          for(size_t j=0;j<4;++j)offsets[j]=(uint16_t)(256-image_threshold[(y0+y)&3][(x0+x+j)&3]);
          uint16x4_t packed=fast_over4(p,vld1_u16(out+y*pitch+x),vld1_u16(offsets));
          vst1_u16(out+y*pitch+x,packed);
#else
          uint32_t pixels[4];
          vst1q_u32(pixels, p);
          for (size_t j = 0; j < 4; ++j)
            out[y * pitch + x + j] = MAGIK_PIXEL(pixels[j], out[y * pitch + x + j], x0 + x + j, y0 + y);
#endif
        }
      }
    }
    for (; x < width; ++x) {
      uint32_t a = r >= 0 && (size_t)r < height ? src[x * stride + r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[x * stride + r + 1] : 0;
      out[y * pitch + x] = MAGIK_PIXEL(scalar(a, b, w), out[y * pitch + x], x0 + x, y0 + y);
    }
  }
}
