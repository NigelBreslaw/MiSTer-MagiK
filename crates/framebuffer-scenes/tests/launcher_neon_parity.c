// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// Native ARM NEON pixel parity; timing is measured separately on the MiSTer.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../src/launcher_texture_neon.c"
static inline uint32x2_t reference_interpolate2(const uint32_t *src, int32_t q0,
                                      int32_t q1) {
  int32_t r0 = q0 >> 16, r1 = q1 >> 16;
  uint32x2x2_t ab = vtrn_u32(vld1_u32(src + r0), vld1_u32(src + r1));
  uint16x8_t a = vmovl_u8(vreinterpret_u8_u32(ab.val[0]));
  uint16x8_t b = vmovl_u8(vreinterpret_u8_u32(ab.val[1]));
  uint16x8_t w = vcombine_u16(vdup_n_u16(((uint32_t)q0 & 65535) >> 8),
                                vdup_n_u16(((uint32_t)q1 & 65535) >> 8));
  return vreinterpret_u32_u8(vshrn_n_u16(
      vmlaq_u16(vmulq_u16(a, vsubq_u16(vdupq_n_u16(256), w)), b, w), 8));
}

void reference_project_dithered(uint16_t *out,size_t pitch,const uint32_t *src,
    size_t height,size_t rows,int32_t q,int32_t step,size_t x,size_t y0) {
  size_t y=0;
  for (;y+1<rows;y+=2) {
    int32_t r=q>>16, q1=q+step, r1=q1>>16;
    if (r>=0 && r1>=0 && (size_t)(r+1)<height && (size_t)(r1+1)<height) {
      uint32x2_t p=reference_interpolate2(src,q,q1);
      uint32x2_t alpha=vshr_n_u32(p,24);
      if (vget_lane_u32(alpha,0)==255 && vget_lane_u32(alpha,1)==255) {
        uint32x2_t packed=pack_dithered2(p,x,y0+y);
        out[y*pitch]=(uint16_t)vget_lane_u32(packed,0);
        out[(y+1)*pitch]=(uint16_t)vget_lane_u32(packed,1);
      } else {
        out[y*pitch]=dither_pixel(vget_lane_u32(p,0),out[y*pitch],x,y0+y);
        out[(y+1)*pitch]=dither_pixel(vget_lane_u32(p,1),out[(y+1)*pitch],x,y0+y+1);
      }
    } else {
      for (size_t j=0;j<2;++j) {
        int32_t qj=q+(int32_t)j*step,rj=qj>>16;
        uint32_t a=rj>=0 && (size_t)rj<height ? src[rj]:0;
        uint32_t b=rj+1>=0 && (size_t)(rj+1)<height ? src[rj+1]:0;
        out[(y+j)*pitch]=dither_pixel(scalar(a,b,((uint32_t)qj&65535)>>8),out[(y+j)*pitch],x,y0+y+j);
      }
    }
    q=q1+step;
  }
  if (y<rows) {
    int32_t r=q>>16;
    uint32_t a=r>=0 && (size_t)r<height ? src[r]:0;
    uint32_t b=r+1>=0 && (size_t)(r+1)<height ? src[r+1]:0;
    out[y*pitch]=dither_pixel(scalar(a,b,((uint32_t)q&65535)>>8),out[y*pitch],x,y0+y);
  }
}


static uint32_t seed=237;
static uint32_t next(void) {seed=seed*1664525+1013904223;return seed;}
int main(void) {
  for(size_t trial=0;trial<40000;++trial) {
    size_t height=1+next()%273,rows=next()%557,pitch=1+next()%17;
    uint32_t src[273];uint16_t a[10000],b[10000];
    for(size_t j=0;j<height;++j) {
      uint32_t alpha=trial%3==0?255:trial%3==1?next()%256:0;
      src[j]=(next()%(alpha+1))|(next()%(alpha+1))<<8|(next()%(alpha+1))<<16|alpha<<24;
    }
    for(size_t j=0;j<10000;++j)a[j]=b[j]=(uint16_t)next();
    int32_t q=(int32_t)(next()%196609)-131072,step=1+next()%196608;
    size_t x=next()%960,y=next()%540;
    reference_project_dithered(a+3,pitch,src,height,rows,q,step,x,y);
    magik_launcher_project_dithered(b+3,pitch,src,height,rows,q,step,x,y);
    if(memcmp(a,b,sizeof a)) {fprintf(stderr,"project mismatch trial %zu\n",trial);return 1;}
  }
  for(uint32_t a=0;a<256;++a)for(uint32_t b=0;b<256;++b)for(uint32_t w=0;w<256;w+=4) {
    uint32_t ws[4]={w,w+1,w+2,w+3},aa=a*0x01010101u,bb=b*0x01010101u,got[4];
    vst1q_u32(got,cabinet_mix4(vdupq_n_u32(aa),vdupq_n_u32(bb),vld1q_u32(ws)));
    for(size_t i=0;i<4;++i)if(got[i]!=scalar(aa,bb,ws[i])) return 2;
  }
  for(size_t trial=0;trial<100000;++trial) {
    uint32_t a[4],b[4],w[4],got[4];
    for(size_t i=0;i<4;++i){a[i]=next();b[i]=next();w[i]=next()%256;}
    vst1q_u32(got,cabinet_mix4(vld1q_u32(a),vld1q_u32(b),vld1q_u32(w)));
    for(size_t i=0;i<4;++i)if(got[i]!=scalar(a[i],b[i],w[i])) return 3;
  }
  for(uint32_t a=0;a<256;++a)for(uint32_t b=0;b<256;++b)for(uint32_t w=0;w<=256;++w) {
    uint32_t aa=a*0x01010101u,bb=b*0x01010101u,got[4];
    vst1q_u32(got,blend(vdupq_n_u32(aa),vdupq_n_u32(bb),w));
    for(size_t i=0;i<4;++i)if(got[i]!=scalar(aa,bb,w)) return 4;
  }
  {
    uint32_t src[273];for(size_t j=0;j<273;++j)src[j]=next();
    for(size_t trial=0;trial<200000;++trial) {
      int32_t q0=(int32_t)(next()%(272*65536)),q1=(int32_t)(next()%(272*65536));
      uint32_t actual[2],expected[2];
      vst1_u32(actual,interpolate2(src,q0,q1));vst1_u32(expected,reference_interpolate2(src,q0,q1));
      if(actual[0]!=expected[0]||actual[1]!=expected[1])return 5;
    }
  }
 for(uint32_t bg=0;bg<65536;++bg)for(uint32_t alpha=0;alpha<256;++alpha) {
  uint32_t p[4];uint16_t dest[4],offset[4],actual[4];size_t x=bg&3,y=(bg^alpha)&3;
  for(size_t j=0;j<4;++j) {
    uint32_t a=(alpha+j*63)&255;uint32_t colour=(bg+j*0x397e51)&0xffffff;
    p[j]=colour|(a<<24);dest[j]=(uint16_t)(bg+j*977);offset[j]=(uint16_t)(256-image_threshold[y][(x+j)&3]);
  }
  vst1_u16(actual,dither_over4(vld1q_u32(p),vld1_u16(dest),vld1_u16(offset)));
  for(size_t j=0;j<4;++j)if(actual[j]!=dither_pixel(p[j],dest[j],x+j,y)) {fprintf(stderr,"over mismatch %u %u %zu\n",bg,alpha,j);return 1;}
 }
  puts("40000 projection cases, 16777216 interpolation triples, 16842752 constant-weight triples and 100000 mixed-lane plus 16777216 final-over NEON cases match exactly");
}
