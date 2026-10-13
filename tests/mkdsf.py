import struct, math, sys
def make(path, secs=3, rate=2822400, ch=2, freq=1000):
    nbits = rate*secs
    nbytes = (nbits+7)//8
    blocks = (nbytes+4095)//4096
    chans=[]
    for c in range(ch):
        out=bytearray(blocks*4096); acc=0.0; y=1.0
        f=freq*(c+1)
        for i in range(nbits):
            x=0.5*math.sin(2*math.pi*f*i/rate)
            acc+=x-y
            y=1.0 if acc>=0 else -1.0
            if y>0: out[i>>3]|=1<<(i&7)  # LSB first
        # fill padding with silence pattern 0x69
        for j in range(nbytes, len(out)): out[j]=0x69
        chans.append(out)
    data=bytearray()
    for b in range(blocks):
        for c in range(ch): data+=chans[c][b*4096:(b+1)*4096]
    fmt=struct.pack('<4sQIIIIIIQII',b'fmt ',52,1,0,2 if ch==2 else ch,ch,rate,1,nbits,4096,0)
    datahdr=struct.pack('<4sQ',b'data',12+len(data))
    total=28+len(fmt)+len(datahdr)+len(data)
    hdr=struct.pack('<4sQQQ',b'DSD ',28,total,0)
    open(path,'wb').write(hdr+fmt+datahdr+data)
make(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]))
