//! Production page filling against an independent zero-and-overlay model.
#![allow(dead_code)]
extern crate self as nun;
pub use a9n_abi::{CapabilityError, Word, TLS_BASE_OFFSET, BYTE_BITS};
use core::ptr;
const PAGE_SIZE: usize = 4096;
#[path = "../../nanami/src/nanami_core/elf_loader.rs"]
mod elf_loader;
use elf_loader::{ElfImage, LoadSegment};
use elf_loader::page::fill_image_page;
#[derive(Clone, Copy)]
enum ProcessLazyMappingKind { Zero, Image { image: &'static [u8], elf: ElfImage } }
fn max_usize(a: usize, b: usize) -> usize { a.max(b) }
fn min_usize(a: usize, b: usize) -> usize { a.min(b) }
#[path = "../../nanami/src/nanami_core/alpha/page_fill.rs"]
mod production;

fn oracle(image: &[u8], elf: ElfImage, page: usize, out: &mut [u8]) -> Result<(), CapabilityError> {
    out.fill(0);
    let end=page.checked_add(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
    for s in &elf.segments[..elf.segment_count] {
        let limit=s.virtual_address.checked_add(s.memory_size).ok_or(CapabilityError::InvalidArgument)?;
        if s.memory_size==0 || page>=limit || end<=s.virtual_address { continue; }
        let file_end=s.virtual_address.checked_add(s.file_size).ok_or(CapabilityError::InvalidArgument)?;
        let first=page.max(s.virtual_address);let last=end.min(file_end);
        if first>=last { continue; }
        let src=s.offset.checked_add(first-s.virtual_address).ok_or(CapabilityError::InvalidArgument)?;
        let len=last-first;
        let source=image.get(src..src.checked_add(len).ok_or(CapabilityError::InvalidArgument)?)
            .ok_or(CapabilityError::InvalidArgument)?;
        for (i,byte) in source.iter().enumerate() { out[first-page+i]=*byte; }
    }
    if let Some(ipc)=elf.ipc_buffer_start {
        let slot=ipc+(TLS_BASE_OFFSET as usize)*BYTE_BITS;
        if slot>=page && slot+core::mem::size_of::<Word>()<=end {
            out[slot-page..slot-page+core::mem::size_of::<Word>()].copy_from_slice(&(ipc as Word).to_ne_bytes());
        }
    }
    Ok(())
}
fn compare(image: &'static [u8], segments: &[LoadSegment], page: usize, ipc: Option<usize>) {
    let mut elf=ElfImage::empty();elf.segment_count=segments.len();elf.segments[..segments.len()].copy_from_slice(segments);elf.ipc_buffer_start=ipc;
    let mut storage=vec![usize::from_ne_bytes([0xa5;core::mem::size_of::<usize>()]);PAGE_SIZE/core::mem::size_of::<usize>()+2];
    let sentinel=storage[0];let address=unsafe { storage.as_mut_ptr().add(1) } as usize;
    let actual=production::fill_lazy_page(&ProcessLazyMappingKind::Image {image,elf},page,address);
    let mut expected=vec![0x75;PAGE_SIZE];let result=oracle(image,elf,page,&mut expected);
    assert!(matches!((actual,result), (Ok(()),Ok(())) |
        (Err(CapabilityError::InvalidArgument),Err(CapabilityError::InvalidArgument))));
    assert_eq!(unsafe { core::slice::from_raw_parts(address as *const u8,PAGE_SIZE) },expected);
    assert_eq!(storage[0],sentinel);assert_eq!(*storage.last().unwrap(),sentinel);
}
fn seg(va:usize,offset:usize,file:usize,mem:usize)->LoadSegment {
    LoadSegment {virtual_address:va,offset,file_size:file,memory_size:mem}
}
fn image()-> &'static [u8] { Box::leak((0..32768).map(|i|((i*17+3)%251) as u8).collect::<Vec<_>>().into_boxed_slice()) }

#[test]
fn parsed_load_flags_preinstall_linux_rights_with_dynamic_bias() {
    // Include ignored high bits and every ELF R/W/X combination. The loader's
    // program-header parsing and page selection both run here.
    for dynamic in [false, true] {
        let mut bytes = vec![0_u8; 64 + 8 * 56];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[16..18].copy_from_slice(&(if dynamic { 3_u16 } else { 2 }).to_le_bytes());
        bytes[18..20].copy_from_slice(&0x3e_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&8_u16.to_le_bytes());
        for flags in 0..8_u32 {
            let h = 64 + flags as usize * 56;
            bytes[h..h+4].copy_from_slice(&1_u32.to_le_bytes());
            bytes[h+4..h+8].copy_from_slice(&(flags | 0x80000000).to_le_bytes());
            bytes[h+16..h+24].copy_from_slice(&(0x10000 + u64::from(flags)*8192).to_le_bytes());
            bytes[h+40..h+48].copy_from_slice(&4096_u64.to_le_bytes());
        }
        let parsed = elf_loader::parse_elf64(&bytes).unwrap();
        let bias = if dynamic { 0x400000 } else { 0 };
        let expected = [7, 4, 2, 6, 1, 5, 3, 7];
        for (index, rights) in expected.into_iter().enumerate() {
            assert_eq!(parsed.spawn_page_permissions(bias+0x10000+index*8192, PAGE_SIZE), rights);
            assert_eq!(parsed.spawn_page_permissions(bias+0x11000+index*8192, PAGE_SIZE), 7);
        }
    }
}

#[test]
fn snapshot_page_accepts_only_exact_readonly_file_pages_and_matches_real_fill() {
    let data=image();let p=0x400000;
    for rights in 0..8_u8 {
        for offset in [0,1,4096] {
            for start in [p,p+17] {
                for file in [0,4095,4096,4097,8192] {
                    let mut elf=ElfImage::empty();elf.segment_count=1;
                    elf.segments[0]=seg(start,offset,file,file+4096);elf.segment_permissions[0]=rights;
                    for page in [p,p+4096,p+8192,p+12288] {
                        let expected=if matches!(rights,1|5) && page>=start
                            && page+4096<=start+file && (offset+page-start)%4096==0
                        {Some((offset+page-start,rights as usize))} else {None};
                        assert_eq!(elf.snapshot_page(page,4096),expected);
                        if let Some((source,_))=expected {
                            let mut out=[0xa5_u8;4096];
                            production::fill_lazy_page(&ProcessLazyMappingKind::Image {image:data,elf},page,out.as_mut_ptr() as usize).unwrap();
                            assert_eq!(&out[..],&data[source..source+4096]);
                        }
                    }
                }
            }
        }
    }
    let mut elf=ElfImage::empty();elf.segment_count=2;
    elf.segments[..2].copy_from_slice(&[seg(p,0,8192,8192),seg(p+17,0,0,1)]);
    elf.segment_permissions[..2].copy_from_slice(&[5,1]);
    assert_eq!(elf.snapshot_page(p,4096),None);
    assert_eq!(elf.snapshot_page(p+4096,4096),Some((4096,5)));
    elf.ipc_buffer_start=Some(p);assert_eq!(elf.snapshot_page(p+4096,4096),None);
    elf.ipc_buffer_start=None;assert_eq!(elf.snapshot_page(usize::MAX-4095,4096),None);
}

#[test]
fn exec_load_permissions_require_sealed_raw_image_and_keep_overlap_fallback() {
    let mut elf=ElfImage::empty();elf.segment_count=2;
    elf.segments[..2].copy_from_slice(&[seg(0x400000,0,8192,8192),seg(0x401800,8192,8192,8192)]);
    elf.segment_permissions[..2].copy_from_slice(&[5,3]);
    for sealed in [false,true] {
        for ipc in [None,Some(0x402000)] {
            elf.ipc_buffer_start=ipc;
            assert_eq!(elf.exec_page_permissions(sealed,0x400000,PAGE_SIZE),
                if sealed && ipc.is_none() {5} else {7});
            assert_eq!(elf.exec_page_permissions(sealed,0x401000,PAGE_SIZE),7);
            assert_eq!(elf.exec_page_permissions(sealed,0x402000,PAGE_SIZE),
                if sealed && ipc.is_none() {3} else {7});
        }
    }
}

#[test]
fn shared_load_boundary_pages_and_zero_rights_keep_supervisor_ordering() {
    let mut elf=ElfImage::empty();elf.segment_count=3;
    elf.segments[..3].copy_from_slice(&[
        seg(0x10017,0,4096,8192), seg(0x12000,0,4096,4096), seg(0x14000,0,0,4096),
    ]);
    elf.segment_permissions[..3].copy_from_slice(&[5,3,0]);
    assert_eq!(elf.spawn_page_permissions(0x10000,PAGE_SIZE),5);
    assert_eq!(elf.spawn_page_permissions(0x11000,PAGE_SIZE),5);
    assert_eq!(elf.spawn_page_permissions(0x12000,PAGE_SIZE),7);
    assert_eq!(elf.spawn_page_permissions(0x13000,PAGE_SIZE),7);
    assert_eq!(elf.spawn_page_permissions(0x14000,PAGE_SIZE),7);
    elf.segments.swap(0,1);elf.segment_permissions.swap(0,1);
    assert_eq!(elf.spawn_page_permissions(0x12000,PAGE_SIZE),7);
    assert_eq!(elf.spawn_page_permissions(usize::MAX-2047,PAGE_SIZE),7);
}

#[test]
fn full_pages_partial_edges_bss_overlaps_and_tls_match_zero_then_copy() {
    let image=image();let p=0x400000;
    for segments in [vec![],vec![seg(p,0,3*PAGE_SIZE,4*PAGE_SIZE)],
        vec![seg(p+17,7,8200,10000)],vec![seg(p,0,0,4*PAGE_SIZE)],
        vec![seg(p,0,3*PAGE_SIZE,3*PAGE_SIZE),seg(p+300,20000,123,700)],
        vec![seg(p+700,10000,31,900),seg(p,0,3*PAGE_SIZE,4*PAGE_SIZE)],
        vec![seg(p+7,1000,8000,12000),seg(p+17,20000,1000,1800)]] {
        for page in [p-PAGE_SIZE,p,p+PAGE_SIZE,p+2*PAGE_SIZE,p+3*PAGE_SIZE] {
            compare(image,&segments,page,None);
            compare(image,&segments,page,Some(p));
        }
    }
}

#[test]
fn failed_fills_keep_the_same_zeroed_or_copied_prefix_and_neighbor_bytes() {
    let image=image();let p=0x400000;
    for prefix in [vec![],vec![seg(p,0,PAGE_SIZE,PAGE_SIZE)],vec![seg(p+37,0,200,PAGE_SIZE)]] {
        for bad in [seg(p,usize::MAX,4096,4096),seg(p,32767,4096,4096),
            seg(usize::MAX-3,0,0,9),seg(p,0,usize::MAX,4096)] {
            let mut segments=prefix.clone();segments.push(bad);compare(image,&segments,p,None);
        }
    }
    compare(image,&[],usize::MAX-2047,None);
    compare(image,&[seg(usize::MAX-8191,0,4096,4096)],usize::MAX-8191,None);
}

#[test]
fn randomized_segment_order_and_boundaries_preserve_all_bytes() {
    let image=image();let p=0x800000;let mut rng=123_u64;
    for _ in 0..2000 {
        let mut segments=Vec::new();
        for _ in 0..8 {
            rng=rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let va=p+((rng>>32) as usize%12000);let size=(rng as usize)%8193;
            segments.push(seg(va,((rng>>16) as usize)%16384,size,size+((rng>>48) as usize)%4096));
        }
        compare(image,&segments,p+PAGE_SIZE,None);
    }
}

#[test]
fn anonymous_page_still_clears_every_byte() {
    let mut page=[usize::MAX;PAGE_SIZE/core::mem::size_of::<usize>()];
    production::fill_lazy_page(&ProcessLazyMappingKind::Zero,0,page.as_mut_ptr() as usize).unwrap();
    assert!(page.iter().all(|v|*v==0));
}

#[test]
fn sparse_private_payload_matches_ordered_fill_with_poisoned_unloaded_bytes() {
    use elf_loader::staging::StagedImage;
    let data = image();
    for segments in [
        vec![seg(0x400000, 0, 24576, 28672)],
        vec![seg(0x400017, 17, 17000, 22000)],
        vec![seg(0x400000, 0, 24576, 28672), seg(0x402017, 28672, 1000, 3000)],
        vec![seg(0x402017, 28672, 1000, 3000), seg(0x400000, 0, 24576, 28672)],
    ] {
        for rights in [1, 3, 5, 7] {
            let mut elf = ElfImage::empty();
            elf.segment_count = segments.len();
            elf.segments[..segments.len()].copy_from_slice(&segments);
            elf.segment_permissions.fill(rights);
            let mut buffer = vec![0xa5; data.len()];
            let mut copied = Vec::new();
            let mut stage = StagedImage::new(&mut buffer, PAGE_SIZE, |offset, dst: &mut [u8]| {
                copied.push(offset);
                dst.copy_from_slice(&data[offset..offset + dst.len()]); Ok(())
            }).unwrap();
            stage.stage_private_pages(&elf).unwrap();
            let staged = Box::leak(stage.finish().to_vec().into_boxed_slice());
            for page in (0x3ff000..0x409000).step_by(PAGE_SIZE) {
                if elf.snapshot_page(page, PAGE_SIZE).is_some() { continue; }
                let mut expected = [0; PAGE_SIZE];
                oracle(data, elf, page, &mut expected).unwrap();
                let mut actual = [0; PAGE_SIZE];
                production::fill_lazy_page(&ProcessLazyMappingKind::Image { image: staged, elf },
                    page, actual.as_mut_ptr() as usize).unwrap();
                assert_eq!(actual, expected);
            }
            let count = copied.len(); copied.sort_unstable(); copied.dedup();
            assert_eq!(copied.len(), count);
            if segments.len() == 1 && segments[0].virtual_address == 0x400000 && rights != 3 && rights != 7 {
                assert!(copied.is_empty(), "all file pages are snapshot eligible");
            }
        }
    }
}

fn sparse_test_elf() -> Vec<u8> {
    let mut image = vec![0; 10 * PAGE_SIZE + 19];
    image[..6].copy_from_slice(b"\x7fELF\x02\x01");
    for (offset, value) in [(16,3u16), (18,0x3e), (54,56), (56,1), (58,64), (60,4)] {
        image[offset..offset+2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(24,0u64), (32,4096), (40,8192),
        (4096+8,12288), (4096+16,0), (4096+32,4096), (4096+40,8192),
        (8192+64+24,16384), (8192+64+32,48), (8192+64+56,24),
        (8192+128+24,20480), (8192+128+32,19), (16384+24+8,0x1234)] {
        image[offset..offset+8].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(4096,1u32),(4096+4,5),(8192+64+4,2),
        (8192+64+40,2),(16384+24,1),
        // A malformed later symbol section must be ignored after an early match.
        (8192+192+4,2)] {
        image[offset..offset+4].copy_from_slice(&value.to_le_bytes());
    }
    image[20481..20499].copy_from_slice(b"__ipc_buffer_start");
    image
}

#[test]
fn sparse_parser_preserves_symbol_early_return_bias_and_all_truncation_errors() {
    use elf_loader::{ElfReader, staging::StagedImage};
    let original = sparse_test_elf();
    let mut variants = vec![original.clone()];
    for offset in [0, 4, 18, 32, 40, 54, 56, 58, 60, 4096+32,
        8192+64+24, 8192+64+32, 8192+64+40, 8192+64+56, 8192+128+24, 20481] {
        let mut bad = original.clone(); bad[offset] ^= 0xff; variants.push(bad);
    }
    for length in [0, 6, 63, 64, 4096+55, 8192+255, 16384+47, 20498] {
        variants.push(original[..length].to_vec());
    }
    for (index, source) in variants.iter().enumerate() {
        let expected = elf_loader::parse_elf64(source);
        let mut buffer = vec![0xa5; source.len()];
        let mut copied = Vec::new();
        let mut stage = StagedImage::new(&mut buffer, PAGE_SIZE, |offset, dst: &mut [u8]| {
            copied.push(offset); dst.copy_from_slice(&source[offset..offset+dst.len()]); Ok(())
        }).unwrap();
        let actual = elf_loader::parse_elf64_reader(&mut stage);
        match (actual, expected) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a.entry_point,b.entry_point); assert_eq!(a.ipc_buffer_start,b.ipc_buffer_start);
                assert_eq!(a.segment_count,b.segment_count); assert_eq!(a.segment_permissions,b.segment_permissions);
                for (a,b) in a.segments.iter().zip(b.segments.iter()) {
                    assert_eq!((a.offset,a.virtual_address,a.file_size,a.memory_size),
                        (b.offset,b.virtual_address,b.file_size,b.memory_size));
                }
                if index == 0 { assert_eq!(a.ipc_buffer_start,Some(0x401234)); }
            }
            (Err(a),Err(b)) => assert_eq!(a as usize,b as usize),
            _ => panic!("sparse/slice mismatch variant {index}"),
        }
        if index == 0 {
            assert!(stage.read(usize::MAX,2).is_err());
            stage.read(0,6).unwrap();
        }
        drop(stage);
        let count = copied.len(); copied.sort_unstable(); copied.dedup(); assert_eq!(count,copied.len());
        if index == 0 { assert_eq!(copied,vec![0,4096,8192,16384,20480]); }
    }
}

#[test]
fn sparse_read_failure_never_marks_partial_data_ready_and_new_requests_reload() {
    use elf_loader::{ElfReader, staging::StagedImage};
    for byte in [0x12,0x34] {
        let mut buffer = vec![0xa5; PAGE_SIZE+3];
        let mut calls = 0;
        let mut stage = StagedImage::new(&mut buffer,PAGE_SIZE,|_,dst: &mut [u8]| {
            calls += 1; dst.fill(byte);
            if calls == 1 { Err(CapabilityError::InvalidDescriptor) } else { Ok(()) }
        }).unwrap();
        assert!(stage.read(0,1).is_err());
        assert_eq!(stage.read(0,1).unwrap(), &[byte]);
        assert_eq!(stage.read(PAGE_SIZE-1,4).unwrap(), &[byte;4]);
        assert_eq!(stage.read(PAGE_SIZE+3,0).unwrap(), &[]);
        assert!(stage.read(PAGE_SIZE+3,1).is_err());
        drop(stage); assert_eq!(calls,3);
    }
    let mut oversized = vec![0;4096*PAGE_SIZE+1];
    assert!(StagedImage::new(&mut oversized,PAGE_SIZE,|_,_: &mut [u8]| Ok(())).is_err());
}

#[test]
fn streamed_page_reads_only_ordered_load_payload_and_matches_independent_fill() {
    let data=image();let p=0x400000;
    for segments in [
        vec![seg(p,0,3*PAGE_SIZE,4*PAGE_SIZE)],
        vec![seg(p+17,7,8200,10000)],
        vec![seg(p,0,0,4*PAGE_SIZE)],
        vec![seg(p,0,3*PAGE_SIZE,4*PAGE_SIZE),seg(p+700,20000,123,2000)],
        vec![seg(p+700,20000,123,2000),seg(p,0,3*PAGE_SIZE,4*PAGE_SIZE)],
    ] {
        let mut elf=ElfImage::empty();elf.segment_count=segments.len();
        elf.segments[..segments.len()].copy_from_slice(&segments);
        for ipc in [None,Some(p)] {
            elf.ipc_buffer_start=ipc;
            for page in (p-PAGE_SIZE..p+5*PAGE_SIZE).step_by(PAGE_SIZE) {
                let mut expected=[0;PAGE_SIZE];oracle(data,elf,page,&mut expected).unwrap();
                let mut actual=[0xa5;PAGE_SIZE+2];let mut reads=Vec::new();
                fill_image_page(&elf,data.len(),page,&mut actual[1..PAGE_SIZE+1],|offset,dst| {
                    reads.push((offset,dst.len()));dst.copy_from_slice(&data[offset..offset+dst.len()]);Ok(())
                }).unwrap();
                assert_eq!(&actual[1..PAGE_SIZE+1],expected.as_slice());assert_eq!(actual[0],0xa5);assert_eq!(actual[PAGE_SIZE+1],0xa5);
                let expected_reads:Vec<_>=segments.iter().filter_map(|s| {
                    let start=page.max(s.virtual_address);let end=(page+PAGE_SIZE).min(s.virtual_address+s.file_size);
                    (start<end && s.memory_size!=0).then(||(s.offset+start-s.virtual_address,end-start))
                }).collect();
                assert_eq!(reads,expected_reads);
            }
        }
    }
}

#[test]
fn streamed_source_failure_stops_before_later_loads_and_keeps_the_initialized_prefix() {
    let data=image();let p=0x400000;let mut elf=ElfImage::empty();elf.segment_count=3;
    elf.segments[..3].copy_from_slice(&[seg(p,0,4096,4096),seg(p+37,10000,200,500),seg(p+300,20000,100,100)]);
    for fail_at in 0..3 {
        let mut calls=0;let mut actual=[0xa5;PAGE_SIZE];let mut expected=[0xa5;PAGE_SIZE];
        for (i,s) in elf.segments[..3].iter().enumerate() {
            if i==fail_at {break;}
            let out=s.virtual_address-p;expected[out..out+s.file_size].copy_from_slice(&data[s.offset..s.offset+s.file_size]);
        }
        let result=fill_image_page(&elf,data.len(),p,&mut actual,|offset,dst| {
            let call=calls;calls+=1;
            if call==fail_at {return Err(CapabilityError::InvalidDescriptor);}
            dst.copy_from_slice(&data[offset..offset+dst.len()]);Ok(())
        });
        assert!(matches!(result,Err(CapabilityError::InvalidDescriptor)));
        assert_eq!(calls,fail_at+1);assert_eq!(actual,expected);
    }
}

#[test]
#[ignore = "requires NANAMI_SPAWN_ELF artifact; work model, not wall time"]
fn real_spawn_streamed_payload_work() {
    use elf_loader::staging::StagedImage;
    let data=std::fs::read(std::env::var("NANAMI_SPAWN_ELF").unwrap()).unwrap();
    let mut buffer=vec![0xa5;data.len()];let mut metadata_bytes=0;let mut metadata_pages=0;
    let mut image=StagedImage::new(&mut buffer,PAGE_SIZE,|offset,dst: &mut [u8]| {
        metadata_bytes+=dst.len();metadata_pages+=1;dst.copy_from_slice(&data[offset..offset+dst.len()]);Ok(())
    }).unwrap();
    let elf=elf_loader::parse_elf64_reader(&mut image).unwrap();assert!(elf.ipc_buffer_start.is_none());
    drop(image);
    let low=elf.segments[..elf.segment_count].iter().filter(|s|s.memory_size!=0)
        .map(|s|s.virtual_address/PAGE_SIZE*PAGE_SIZE).min().unwrap();
    let high=elf.segments[..elf.segment_count].iter().filter(|s|s.memory_size!=0)
        .map(|s|(s.virtual_address+s.memory_size).div_ceil(PAGE_SIZE)*PAGE_SIZE).max().unwrap();
    let mut copied=0;let mut source_spans=0;
    for page in (low..high).step_by(PAGE_SIZE) {
        let mut expected=[0;PAGE_SIZE];oracle(&data,elf,page,&mut expected).unwrap();
        let mut actual=[0xa5;PAGE_SIZE];
        fill_image_page(&elf,data.len(),page,&mut actual,|offset,dst| {
            copied+=dst.len();source_spans+=1;dst.copy_from_slice(&data[offset..offset+dst.len()]);Ok(())
        }).unwrap();assert_eq!(actual,expected);
    }
    std::println!("SPAWN-STREAM-MODEL file-bytes={} metadata-bytes={} metadata-pages={} staging-avoided={} target-payload-bytes={} source-spans={} target-pages={} bytes-match=true",data.len(),metadata_bytes,metadata_pages,data.len()-metadata_bytes,copied,source_spans,(high-low)/PAGE_SIZE);
}
