// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
#include <stdio.h>
#include <string.h>
#include "../src/launcher_texture_neon.c"
static uint32_t seed=0x347ac237;
static uint32_t next(void){seed=seed*1664525u+1013904223u;return seed;}
static uint32_t reference_mix(uint32_t a,uint32_t b,uint32_t w){
  uint32_t result=0;for(unsigned c=0;c<4;++c)result|=((((a>>(c*8))&255)*(256-w)+((b>>(c*8))&255)*w)>>8)<<(c*8);return result;
}
int main(void){
  for(size_t trial=0;trial<100000;++trial){
    uint32_t source[8][65],actual[67],expected[67];size_t n=next()%66;
    uint32_t wx=next()%257,wx2=next()%257,lod=next()%257,weight=next()%257,light=next()%257;
    for(size_t j=0;j<8;++j)for(size_t i=0;i<65;++i)source[j][i]=next();
    for(size_t i=0;i<67;++i)actual[i]=expected[i]=0xdeadbeefu;
    for(size_t i=0;i<n;++i){
      uint32_t a=reference_mix(source[0][i],source[1][i],wx);
      if(lod)a=reference_mix(a,reference_mix(source[2][i],source[3][i],wx2),lod);
      if(weight){uint32_t c=reference_mix(source[4][i],source[5][i],wx);if(lod)c=reference_mix(c,reference_mix(source[6][i],source[7][i],wx2),lod);a=reference_mix(a,c,weight);}
      uint32_t result=a&0xff000000;for(unsigned c=0;c<3;++c)result|=((((a>>(c*8))&255)*light)>>8)<<(c*8);expected[i+1]=result;
    }
    magik_launcher_filter_blend_shade_column(actual+1,source[0],source[1],source[2],source[3],source[4],source[5],source[6],source[7],n,wx,wx2,lod,weight,light);
    if(memcmp(actual,expected,sizeof actual)){fprintf(stderr,"fusion pixel mismatch at trial %zu\n",trial);return 1;}
  }
  puts("100000 fused column cases match the independent scalar oracle, including guards");return 0;
}
