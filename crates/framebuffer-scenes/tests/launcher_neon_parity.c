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
#ifdef MAGIK_FAST_QUANTISATION
static int fast_quantisation_parity(void) {
  for(size_t trial=0;trial<4096;++trial) {
    size_t h=17+next()%128,rows=1+next()%127,pitch=1+next()%7,x=next()%960,y0=next()%540;
    uint32_t src[160];uint16_t a[1000],b[1000];
    for(size_t j=0;j<h;++j) {
      uint32_t alpha=trial%2?(j>=8 && j<h-8?255:next()%256):next()%256;
      src[j]=next()%(alpha+1)|(next()%(alpha+1))<<8|(next()%(alpha+1))<<16|alpha<<24;
    }
    for(size_t j=0;j<1000;++j)a[j]=b[j]=(uint16_t)next();
    int32_t q=(int32_t)(next()%196609)-131072,step=1+next()%196608;
    for(size_t j=0;j<rows;++j) {
      int32_t qq=q+(int32_t)j*step,r=qq>>16;
      uint32_t p=r>=0&&(size_t)r<h?src[r]:0,z=r+1>=0&&(size_t)(r+1)<h?src[r+1]:0;
      a[3+j*pitch]=fast_dither_pixel(scalar(p,z,((uint32_t)qq&65535)>>8),a[3+j*pitch],x,y0+j);
    }
    magik_launcher_project_dithered_fast_opaque(b+3,pitch,src,h,rows,q,step,x,y0,trial%2?8:0,trial%2?h-8:0);
    if(memcmp(a,b,sizeof(a))) {fprintf(stderr,"fast projection mismatch %zu\n",trial);return 1;}
  }
  // Flat-card gathers, mixed alpha, odd widths and independent output guards.
  for(size_t trial=0;trial<1024;++trial) {
    size_t height=1+next()%63,width=1+next()%17,rows=1+next()%31,pitch=width+3;
    uint32_t source[17*63];uint16_t a[700],b[700];
    for(size_t j=0;j<width*height;++j)source[j]=next();
    for(size_t j=0;j<700;++j)a[j]=b[j]=(uint16_t)next();
    int32_t q=(int32_t)(next()%196609)-131072,step=1+next()%196608;
    size_t x=next()%960,y0=next()%540;
    for(size_t y=0;y<rows;++y) {
      int32_t yy=q+(int32_t)y*step,r=yy>>16;uint32_t w=((uint32_t)yy&65535)>>8;
      for(size_t xx=0;xx<width;++xx) {
        uint32_t p=r>=0&&(size_t)r<height?source[xx*height+r]:0;
        uint32_t z=r+1>=0&&(size_t)(r+1)<height?source[xx*height+r+1]:0;
        size_t at=3+y*pitch+xx;a[at]=fast_dither_pixel(scalar(p,z,w),a[at],x+xx,y0+y);
      }
    }
    magik_launcher_flat_dithered_fast(b+3,pitch,source,height,height,width,rows,q,step,x,y0);
    if(memcmp(a,b,sizeof(a))) {fprintf(stderr,"fast flat mismatch %zu\n",trial);return 22;}
  }
  puts("1024 flat-card alpha, bounds, odd-width and guard cases match scalar");
  puts("Fast dither: 4096 projection cases including alpha, bounds, stride and tails passed");
  return 0;
}
#endif
int main(void) {
#ifdef MAGIK_FAST_QUANTISATION
  if (fast_quantisation_parity()) return 1;
#endif
  for(size_t trial=0;trial<4096;++trial) {
    uint32_t src[8][69],a[69],b[69],fused[69];
    for(size_t c=0;c<8;++c)for(size_t j=0;j<69;++j)src[c][j]=next();
    for(size_t j=0;j<69;++j)a[j]=b[j]=fused[j]=0x9a7bc5d1u;
    size_t n=trial%66,offset=trial%2;
    uint32_t wx=next()%257,wx2=next()%257,lod=next()%257,v=next()%257;
    magik_launcher_filter_column(a+2,src[0]+offset,src[1]+offset,src[2]+offset,src[3]+offset,n,wx,wx2,lod);
    magik_launcher_filter_column(b+2,src[4]+offset,src[5]+offset,src[6]+offset,src[7]+offset,n,wx,wx2,lod);
    magik_launcher_mix_rgba(a+2,b+2,n,v);
    magik_launcher_filter_column_axes(fused+2,src[0]+offset,src[1]+offset,src[2]+offset,src[3]+offset,
      src[4]+offset,src[5]+offset,src[6]+offset,src[7]+offset,n,wx,wx2,lod,v);
    if(memcmp(a,fused,sizeof a)) {fprintf(stderr,"fused axes mismatch trial %zu\n",trial);return 20;}
  }

  size_t opaque_cases=0,opaque_rows=0;
  for(size_t trial=0;trial<40000;++trial) {
    size_t height=1+next()%273,rows=next()%557,pitch=1+next()%17;
    uint32_t src[273];uint16_t a[10000],b[10000];
    for(size_t j=0;j<height;++j) {
      // Fully opaque, rounded caps with an opaque interior, arbitrary alpha,
      // and transparent columns all receive independent backgrounds.
      uint32_t alpha=trial%4==0?255:trial%4==1?
        (height>16 && j>=8 && j<height-8?255:next()%255):trial%4==2?next()%256:0;
      src[j]=(next()%(alpha+1))|(next()%(alpha+1))<<8|(next()%(alpha+1))<<16|alpha<<24;
    }
    for(size_t j=0;j<10000;++j)a[j]=b[j]=(uint16_t)next();
    int32_t q=(int32_t)(next()%196609)-131072,step=1+next()%196608;
    size_t x=next()%960,y=next()%540;
    reference_project_dithered(a+3,pitch,src,height,rows,q,step,x,y);
    magik_launcher_project_dithered(b+3,pitch,src,height,rows,q,step,x,y);
    if(memcmp(a,b,sizeof a)) {fprintf(stderr,"project mismatch trial %zu\n",trial);return 1;}
    // Reinitialize reference and candidate with the same arbitrary background.
    for(size_t j=0;j<10000;++j)a[j]=b[j]=(uint16_t)next();
    reference_project_dithered(a+3,pitch,src,height,rows,q,step,x,y);
    size_t top=trial%4<2 && height>16?8:0;
    size_t bottom=top?height-8:0;
    magik_launcher_project_dithered_opaque(b+3,pitch,src,height,rows,q,step,x,y,top,bottom);
    if(memcmp(a,b,sizeof a)) {fprintf(stderr,"opaque project mismatch trial %zu\n",trial);return 6;}
    // Count valid bilinear interior rows independently of the kernel's
    // interval helper. A >=4-row interval exercises its opaque NEON loop.
    size_t interior=0;
    if(top)for(size_t j=0;j<rows;++j) {
      int64_t row=((int64_t)q+(int64_t)j*step)>>16;
      if(row>=(int64_t)top && row+1<(int64_t)bottom)++interior;
    }
    if(interior>=4) {++opaque_cases;opaque_rows+=interior;}
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
#ifdef MAGIK_FAST_QUANTISATION
  vst1_u16(actual,fast_over4(vld1q_u32(p),vld1_u16(dest),vld1_u16(offset)));
  for(size_t j=0;j<4;++j)if(actual[j]!=fast_dither_pixel(p[j],dest[j],x+j,y)) {fprintf(stderr,"fast over mismatch %u %u %zu\n",bg,alpha,j);return 21;}
#endif
 }
  if(opaque_cases<1000) {fputs("insufficient opaque fast-path coverage\n",stderr);return 7;}
  printf("%zu opaque vector-loop cases covering %zu interior rows\n",opaque_cases,opaque_rows);
  puts("40000 projection cases, 16777216 interpolation triples, 16842752 constant-weight triples and 100000 mixed-lane plus 16777216 final-over NEON cases match exactly");
}
