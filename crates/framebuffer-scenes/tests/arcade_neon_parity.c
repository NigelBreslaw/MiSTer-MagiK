// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../src/rgb565_neon.c"
void reference_arcade_base(uint16_t *out,const uint16_t *home,const uint16_t *arcade,uint16_t a,uint16_t b,size_t y0,size_t y1) {
    const size_t cuts[5]={0,26,488,490,960};
    const uint16x8_t black=vdupq_n_u16(0);
    for(size_t y=y0;y<y1;++y) {
        for(size_t span=0;span<4;++span) {
            int subject=(y>=77 && y<500 && span==3) || (y>=88 && y<484 && span==1);
            size_t i=y*960+cuts[span],end=y*960+cuts[span+1];
            if(y<77) { for(;i<end;++i) out[i]=arcade[i];continue; }
            for(;i+7<end;i+=8) {
                uint16x8_t base=blend8(black,vld1q_u16(home+i),a);
                uint16x8_t chrome=subject?black:vld1q_u16(arcade+i);
                vst1q_u16(out+i,blend8(base,chrome,b));
            }
            for(;i<end;++i) out[i]=blend1(blend1(0,home[i],a),subject?0:arcade[i],b);
        }
    }
}
int main(void) {
 size_t n=960*540; uint16_t *home=malloc(n*2),*arcade=malloc(n*2),*a=malloc(n*2),*b=malloc(n*2);
 for(size_t i=0;i<n;++i) {home[i]=(uint16_t)(i*997);arcade[i]=(uint16_t)(i*877+63);}
 for(uint16_t wa=0;wa<=32;++wa) for(uint16_t wb=0;wb<=32;++wb) {
  memset(a,0x56,n*2);memset(b,0x56,n*2);
  size_t y0=(wa*17+wb*19)%540,y1=y0+83;if(y1>540)y1=540;
  if((wa==0||wa==32)&&(wb==0||wb==32)){y0=0;y1=540;}
  reference_arcade_base(a,home,arcade,wa,wb,y0,y1);
  mister_magik_arcade_base(b,home,arcade,wa,wb,y0,y1);
  if(memcmp(a,b,n*2)) {fprintf(stderr,"base mismatch %u %u\n",wa,wb);return 1;}
 }
 for(uint16_t alpha=0;alpha<=32;++alpha) for(size_t length=0;length<=65;++length) {
  for(size_t i=0;i<100;++i)a[i]=b[i]=(uint16_t)(i*797);
  for(size_t i=0;i<length;++i)a[i+1]=blend1(a[i+1],arcade[i],alpha);
  mister_magik_arcade_over(b+1,arcade,length,alpha);
  if(memcmp(a,b,200))return 2;
 }
 puts("1089 RGB565 fade pairs exactly match, including untouched rows");
 free(home);free(arcade);free(a);free(b);
}
